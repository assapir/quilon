// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! A cooperative, single-threaded fiber scheduler — the bedrock of Quilon's
//! concurrency model (colorless implicit futures). [`spawn`] creates a stackful
//! `corosensei` fiber and enqueues it; [`run`] drives a ready-queue + reactor loop
//! that resumes fibers until each finishes or parks, blocks the [`Reactor`] until
//! the nearest wake deadline, and wakes due fibers. [`sleep`] is the first yield
//! primitive: called from inside a fiber, it parks the fiber with a deadline and
//! yields to the scheduler.
//!
//! The subtle fiber-stack GC scanning lives in [`crate::gc`]. [`__run_fiber_main`] is the
//! C-ABI wrapper the generated `main` calls to run a program's entry on this scheduler, so
//! the `@` leaf IO primitives (e.g. `core.time`'s `@sleep`) have a fiber to park on.

use crate::gc;
use crate::reactor::Reactor;
use corosensei::stack::{DefaultStack, Stack};
use corosensei::{Coroutine, CoroutineResult, Yielder};
use mio::event::Source;
use mio::{Interest, Token};
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Stack size for a spawned fiber. Small, because fibers are many and cheap.
///
/// Keep it a page multiple (512 KiB divides every common page size). The writable region
/// then reaches `base()` exactly, so the GC scan range `[limit + page, base)` has no
/// PROT_NONE gap for `GC_push_all_eager` to fault on.
const FIBER_STACK_SIZE: usize = 512 * 1024;

/// Stack size for the seed fiber, the one [`__run_fiber_main`] runs `^` on. The usual
/// process stack default, so `^` recurses about as deeply as it would on one.
///
/// The seed carries the whole user call tree, so it is much larger than a spawned fiber's
/// stack. A fiber that runs out of stack dies on its guard page with a bare SIGSEGV.
///
/// A fixed size, not the process `RLIMIT_STACK`. The collector pushes a parked fiber's
/// whole registered range, so a scan costs what the stack MEASURES, not what it uses: on
/// aarch64, ~0.06 ms at 512 KiB, ~0.9 ms at 8 MiB, ~7 ms at 64 MiB. A raised `ulimit -s`
/// would buy a slower collector. For depth beyond this, a self-tail-call is lowered to a
/// loop and runs in constant stack.
///
/// The ceiling: a seed fiber that parks (`@readStdin` and `@tcpRequest` park in `force`)
/// costs ~0.9 ms per collection while parked. Scanning it from its suspended stack pointer
/// would fix that. corosensei does not expose the pointer, but the parking helpers below
/// run on the fiber and could record it.
const SEED_STACK_SIZE: usize = 8 * 1024 * 1024;

/// What a fiber yields to the scheduler when it parks.
enum Park {
    /// Park until `Instant`, then become ready again.
    Sleep(Instant),
    /// Park until the reactor reports this token ready. The interest was already
    /// (re)registered by the caller before it parked, so the scheduler only has to
    /// map the token back to this fiber when it fires. Source-agnostic: a socket
    /// today, files/pipes later.
    Readiness(Token),
    /// Park until another fiber wakes this address. A general one-fiber-waits-for-another
    /// rendezvous keyed by an opaque `usize`: [`wake_address`] re-readies every fiber parked
    /// on it. Backs both forcing a deferred value (the address is the deferred cell) and the
    /// single-reader stdin gate (the address is a fixed sentinel). The scheduler only ever
    /// compares the address; it never dereferences it.
    Waiting(usize),
    /// A test case ended: yielded by [`abort_current_case`] from wherever a failing `expect`
    /// is reached, however deeply nested inside the case's own call tree. Only ever yielded
    /// by a coroutine [`run_case_guarded`] resumes, and only ever seen by that function's own
    /// loop — it never reaches [`run`]'s.
    CaseAborted,
    /// An `aborts()` lambda ended in a fail-loud exit: yielded by [`abort_current_trap`] from
    /// wherever that exit is reached (an `assert`/runtime fault via `report::fail_at`, or a
    /// raw `__exit`), carrying the exit code and the report text withheld from stderr. Only
    /// ever yielded by a coroutine [`run_abort_trap_guarded`] resumes, and only ever seen by
    /// that function's own loop — it never reaches [`run`]'s.
    AbortTrapped(c_int, String),
}

type FiberCoroutine = Coroutine<(), Park, (), DefaultStack>;
type FiberYielder = Yielder<(), Park>;

struct Fiber {
    coroutine: FiberCoroutine,
    /// The fiber's stack base (top of its GC-scannable range); set as Boehm's stack
    /// bottom while this fiber runs. The full range is mirrored in the GC registry.
    stack_high: usize,
}

struct Scheduler {
    /// Slab of live fibers indexed by id; `None` marks a free slot.
    fibers: Vec<Option<Fiber>>,
    free: Vec<usize>,
    ready: VecDeque<usize>,
    /// Parked-on-sleep fibers: `(wake deadline, id)`.
    timers: Vec<(Instant, usize)>,
    /// Fibers parked on source readiness, keyed by the token they wait on. Exactly
    /// one fiber owns a token at a time (it owns the source), so this is 1:1.
    readiness_waiters: HashMap<Token, usize>,
    /// Fibers parked on an address (a deferred cell, or the stdin gate). More than one
    /// fiber may wait on the same address, so this is 1:many — every waiter is
    /// re-readied when the address is woken.
    address_waiters: HashMap<usize, Vec<usize>>,
}

impl Scheduler {
    fn new() -> Self {
        Scheduler {
            fibers: Vec::new(),
            free: Vec::new(),
            ready: VecDeque::new(),
            timers: Vec::new(),
            readiness_waiters: HashMap::new(),
            address_waiters: HashMap::new(),
        }
    }

    /// Claim a fresh id from the slab every fiber gets one from, its slot left empty until
    /// the caller fills it. [`run_case_guarded`] uses this directly: its coroutine is
    /// resumed by its own loop rather than through the ready queue, so it never occupies
    /// `fibers[id]` — it only needs the id so its GC registration (keyed by id, same as
    /// every fiber's) cannot collide with one.
    fn reserve_id(&mut self) -> usize {
        if let Some(id) = self.free.pop() {
            id
        } else {
            self.fibers.push(None);
            self.fibers.len() - 1
        }
    }

    fn release_id(&mut self, id: usize) {
        self.fibers[id] = None;
        self.free.push(id);
    }
}

thread_local! {
    /// The active scheduler for this thread. `spawn`/`sleep` reach it here. Borrowed
    /// only in short scopes on the scheduler's own turns — never held across a
    /// `resume`, so a resumed fiber may re-enter (e.g. call `spawn`) freely.
    static SCHEDULER: RefCell<Option<Scheduler>> = const { RefCell::new(None) };

    /// The running fiber's `Yielder`, so the free-standing `sleep` can suspend
    /// without threading the yielder through every call. Set on fiber entry and
    /// re-set by `sleep` after each resume (other fibers run in between and clobber
    /// this shared cell).
    static CURRENT_YIELDER: Cell<*const FiberYielder> = const { Cell::new(ptr::null()) };

    /// The reactor for this thread's run. Lives here (not just as a `run` local) so
    /// readiness ops in [`crate::net`], executing inside a fiber, can register and
    /// (re)register their sources with the same `Poll` the scheduler waits on.
    static REACTOR: RefCell<Option<Reactor>> = const { RefCell::new(None) };

    /// How many `aborts()` traps are currently in progress on this thread — incremented
    /// before [`run_abort_trap_guarded`] resumes its nested fiber for the first time,
    /// decremented once that fiber has finished or aborted. A plain counter rather than a
    /// stack: nesting (a trap inside a case inside another trap) only needs to know
    /// whether SOME trap is active, since a fail-loud exit always suspends whichever fiber
    /// is actually running (the innermost one), caught by that fiber's own guard loop.
    static ABORT_TRAP_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Run `f` against the active scheduler. A short borrow only — never held across a
/// `resume`, so a resumed fiber may re-enter the scheduler freely.
fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    SCHEDULER.with(|s| f(s.borrow_mut().as_mut().expect("no active scheduler")))
}

/// The calling fiber's `Yielder`, asserting one is set. Every free-standing park primitive
/// below, plus [`run_case_guarded`], starts this way rather than repeating the same
/// get-and-assert; `name` names the caller for the panic message.
fn current_yielder(name: &str) -> *const FiberYielder {
    let yielder = CURRENT_YIELDER.get();
    assert!(!yielder.is_null(), "{name}() called outside a fiber");
    yielder
}

/// Suspend `yielder` with `park`, then restore `CURRENT_YIELDER` to it: sibling fibers run
/// between the suspend and its resume and clobber the shared cell (see `CURRENT_YIELDER`'s
/// own doc), so later code on this fiber needs it put back.
fn suspend_on(yielder: *const FiberYielder, park: Park) {
    // SAFETY: `yielder` points at the live `Yielder` for this fiber, valid for the whole
    // fiber body (it is a parameter of the corosensei closure we are inside).
    unsafe { (*yielder).suspend(park) };
    CURRENT_YIELDER.set(yielder);
}

/// Resume `coroutine` (fiber `id`, stack base `high`) with Boehm's stack bottom pointed at
/// it for the resume's duration, restoring whatever it covered before once the coroutine
/// yields or returns. [`run`]'s ready-queue loop and [`run_case_guarded`]'s both resume this
/// way — a fiber is a fiber to the collector whichever loop is driving it.
fn resume_fiber(
    id: usize,
    high: usize,
    coroutine: &mut FiberCoroutine,
) -> CoroutineResult<Park, ()> {
    gc::enter_fiber(id, high);
    let result = coroutine.resume(());
    gc::leave_fiber();
    result
}

/// A freshly allocated fiber stack, and the usable GC-scannable range `[low, high)` within
/// it. [`spawn_with_stack`] and [`run_case_guarded`]'s nested coroutine both need exactly
/// this — the range is what [`gc::register`] tracks by fiber id, so computing it once here
/// is what keeps both callers registering the same way.
struct FiberStackAllocation {
    stack: DefaultStack,
    low: usize,
    high: usize,
}

fn allocate_fiber_stack(stack_size: usize) -> FiberStackAllocation {
    let stack = DefaultStack::new(stack_size).expect("failed to allocate fiber stack");
    let base = stack.base().get();
    let limit = stack.limit().get();
    // Usable region is [limit + guard_page, base); the guard page sits at the low
    // end of the mapping. Scanning from just above it never faults on PROT_NONE.
    FiberStackAllocation {
        stack,
        low: limit + page_size(),
        high: base,
    }
}

/// Build a coroutine on `stack` that installs its own `Yielder` into `CURRENT_YIELDER` as
/// its first act, then runs `body` — the entry every fiber shares, whether driven by the
/// ready queue ([`spawn_with_stack`]) or resumed directly by its own guard
/// ([`run_case_guarded`]).
fn new_fiber(stack: DefaultStack, body: impl FnOnce() + 'static) -> FiberCoroutine {
    Coroutine::with_stack(stack, move |yielder, ()| {
        CURRENT_YIELDER.with(|c| c.set(yielder as *const FiberYielder));
        body();
    })
}

/// Spawn `f` as a new fiber and enqueue it, with a stack of `stack_size` bytes.
fn spawn_with_stack<F: FnOnce() + 'static>(stack_size: usize, f: F) {
    let allocation = allocate_fiber_stack(stack_size);
    let (low, high) = (allocation.low, allocation.high);
    let coroutine = new_fiber(allocation.stack, f);

    SCHEDULER.with(|s| {
        let mut slot = s.borrow_mut();
        let scheduler = slot
            .as_mut()
            .expect("spawn() called with no active scheduler");
        let id = scheduler.reserve_id();
        scheduler.fibers[id] = Some(Fiber {
            coroutine,
            stack_high: high,
        });
        scheduler.ready.push_back(id);
        gc::register(id, low, high);
    });
}

/// Spawn `f` as a child fiber, with the standard [`FIBER_STACK_SIZE`] stack. Call it from
/// within a running fiber; [`run`] seeds the program's own fiber directly, at
/// [`SEED_STACK_SIZE`]. Panics if no scheduler is active.
pub fn spawn<F: FnOnce() + 'static>(f: F) {
    spawn_with_stack(FIBER_STACK_SIZE, f);
}

/// Run a test case's body — `function(environment)`, the raw parts of its `() -> $`
/// closure — to completion on a fresh nested fiber, resumed synchronously right here rather
/// than through the ready queue (corosensei supports resuming a coroutine from inside
/// another one's own execution). Yields whether [`abort_current_case`] ended the case early.
///
/// A park the body causes (`@sleep` and the rest) is forwarded to the fiber calling this —
/// suspended with the very same [`Park`] value — so the scheduler keeps driving it exactly
/// as it would a top-level fiber's; once that outer fiber is resumed in turn, the case
/// fiber is resumed right back. Must be called from within a fiber (asserts otherwise).
pub(crate) fn run_case_guarded(
    function: extern "C" fn(*mut c_void) -> u8,
    environment: *mut c_void,
) -> bool {
    let outer_yielder = current_yielder("run_case_guarded");

    let allocation = allocate_fiber_stack(FIBER_STACK_SIZE);
    let (low, high) = (allocation.low, allocation.high);
    let mut coroutine = new_fiber(allocation.stack, move || {
        function(environment);
    });
    let id = with_scheduler(|scheduler| scheduler.reserve_id());
    gc::register(id, low, high);

    let aborted = loop {
        match resume_fiber(id, high, &mut coroutine) {
            CoroutineResult::Yield(Park::CaseAborted) => break true,
            CoroutineResult::Yield(park) => suspend_on(outer_yielder, park),
            CoroutineResult::Return(()) => break false,
        }
    };
    // The loop above only ever restores `CURRENT_YIELDER` to `outer_yielder` on a forwarded
    // park (`suspend_on`'s own job); ending the case (`CaseAborted`) or finishing normally
    // (`Return`) leaves it pointing at the case coroutine's own `Yielder` instead — about to
    // be freed below. Put it back before this function hands control back to the outer
    // fiber, or its next park dereferences a dangling pointer.
    CURRENT_YIELDER.set(outer_yielder);

    if aborted {
        // The only frames left on the case's stack are Quilon frames (no destructors to
        // run) and the reporter that yielded the abort marker, whose failure it had already
        // moved into the registry before doing so — abandoning them here is safe. A Rust
        // runtime function that held onto heap while calling back into Quilon code that can
        // `expect` would leak here, and none exists today.
        unsafe { coroutine.force_reset() };
    }
    // Unregister before dropping, which unmaps the stack: never leave a range in the GC
    // registry that points at freed memory (mirrors `run`'s own finished-fiber teardown).
    gc::unregister(id);
    drop(coroutine);
    with_scheduler(|scheduler| scheduler.release_id(id));

    aborted
}

/// End the currently running test case: suspend it with the abort marker (see
/// [`Park::CaseAborted`]), through the same thread-local yielder [`sleep`] uses. Never
/// returns — [`run_case_guarded`]'s loop force-resets this coroutine once it sees the
/// marker, so it is never resumed again. Must be called from within a case's own fiber (a
/// failing `expect` only ever reaches this from inside one — the type checker enforces it).
pub(crate) fn abort_current_case() -> ! {
    let yielder = current_yielder("abort_current_case");
    // SAFETY: `yielder` points at the live `Yielder` for this fiber (see `sleep`).
    unsafe { (*yielder).suspend(Park::CaseAborted) };
    unreachable!("run_case_guarded force-resets this coroutine on the abort marker")
}

/// Whether an `aborts()` trap is currently in progress on this thread — checked by
/// `report::fail_at` and `process::__exit` to decide whether a fail-loud exit is withheld
/// and trapped instead of terminating the process.
pub(crate) fn abort_trap_active() -> bool {
    ABORT_TRAP_DEPTH.get() > 0
}

/// Run an `aborts()` lambda's body — `function(environment)`, the raw parts of its
/// (possibly wrapped) closure — to completion on a fresh nested fiber, resumed
/// synchronously right here exactly as [`run_case_guarded`] resumes a case's. Yields the
/// outcome: whether [`abort_current_trap`] ended it early, and if so, what it recorded.
///
/// A park the body causes (`@sleep` and the rest) is forwarded to the fiber calling this,
/// the same way [`run_case_guarded`] forwards one, so the scheduler keeps driving it as it
/// would a top-level fiber's. Must be called from within a fiber (asserts otherwise).
pub(crate) fn run_abort_trap_guarded(
    function: extern "C" fn(*mut c_void) -> u8,
    environment: *mut c_void,
) -> Option<(c_int, String)> {
    let outer_yielder = current_yielder("run_abort_trap_guarded");

    let allocation = allocate_fiber_stack(FIBER_STACK_SIZE);
    let (low, high) = (allocation.low, allocation.high);
    let mut coroutine = new_fiber(allocation.stack, move || {
        function(environment);
    });
    let id = with_scheduler(|scheduler| scheduler.reserve_id());
    gc::register(id, low, high);

    ABORT_TRAP_DEPTH.set(ABORT_TRAP_DEPTH.get() + 1);
    let outcome = loop {
        match resume_fiber(id, high, &mut coroutine) {
            CoroutineResult::Yield(Park::AbortTrapped(exit_code, report)) => {
                break Some((exit_code, report));
            }
            CoroutineResult::Yield(park) => suspend_on(outer_yielder, park),
            CoroutineResult::Return(()) => break None,
        }
    };
    ABORT_TRAP_DEPTH.set(ABORT_TRAP_DEPTH.get() - 1);
    // See `run_case_guarded`'s own comment at the identical line: put the outer yielder
    // back before returning control to it, or its next park dereferences a dangling one.
    CURRENT_YIELDER.set(outer_yielder);

    if outcome.is_some() {
        // Safe for the same reason `run_case_guarded` force-resets its own coroutine: the
        // only frames left on the trap's stack are Quilon frames (no destructors) and the
        // fail-loud reporter, whose outcome it already moved into the `Park` value before
        // suspending.
        unsafe { coroutine.force_reset() };
    }
    gc::unregister(id);
    drop(coroutine);
    with_scheduler(|scheduler| scheduler.release_id(id));

    outcome
}

/// End the currently running `aborts()` trap: suspend it with the abort marker (see
/// [`Park::AbortTrapped`]), carrying `exit_code` and the report withheld from stderr,
/// through the same thread-local yielder [`sleep`] uses. Never returns —
/// [`run_abort_trap_guarded`]'s loop force-resets this coroutine once it sees the marker,
/// so it is never resumed again. Must be called from within a trapped fiber (a fail-loud
/// exit only ever reaches this while [`abort_trap_active`] is true).
pub(crate) fn abort_current_trap(exit_code: c_int, report: String) -> ! {
    let yielder = current_yielder("abort_current_trap");
    // SAFETY: `yielder` points at the live `Yielder` for this fiber (see `sleep`).
    unsafe { (*yielder).suspend(Park::AbortTrapped(exit_code, report)) };
    unreachable!("run_abort_trap_guarded force-resets this coroutine on the abort marker")
}

/// Park the current fiber until `duration` elapses, yielding to the scheduler. Must
/// be called from within a fiber (panics otherwise).
pub fn sleep(duration: Duration) {
    let yielder = current_yielder("sleep");
    let deadline = Instant::now() + duration;
    suspend_on(yielder, Park::Sleep(deadline));
}

/// Park the current fiber until the reactor reports `token` ready, yielding to the
/// scheduler. The caller ([`crate::net`]) must have (re)registered the source for the
/// interest it needs *before* calling this, so the readiness that wakes it is the one
/// it is waiting for. Must be called from within a fiber (panics otherwise).
pub(crate) fn park_on_readiness(token: Token) {
    let yielder = current_yielder("park_on_readiness");
    suspend_on(yielder, Park::Readiness(token));
}

/// Park the current fiber until another fiber wakes `address`. The caller re-checks its own
/// condition after every wake (a wake is an invitation to look, never a guarantee), so a
/// spurious or shared wake simply re-parks. Must be called from within a fiber (panics
/// otherwise).
pub(crate) fn park_on_address(address: usize) {
    let yielder = current_yielder("park_on_address");
    suspend_on(yielder, Park::Waiting(address));
}

/// Re-ready every fiber parked on `address`. Called from the fiber that just made the waited
/// condition true (a deferred fulfilled, the stdin gate released). A no-op if nothing waits.
pub(crate) fn wake_address(address: usize) {
    with_scheduler(|scheduler| {
        if let Some(waiters) = scheduler.address_waiters.remove(&address) {
            for id in waiters {
                scheduler.ready.push_back(id);
            }
        }
    });
}

/// Allocate a token and register `source` with the active reactor for `interest`.
pub(crate) fn register_readiness(
    source: &mut impl Source,
    interest: Interest,
) -> io::Result<Token> {
    with_reactor(|reactor| {
        let token = reactor.alloc_token();
        reactor.register(source, token, interest)?;
        Ok(token)
    })
}

/// Change the interest `source` (already registered under `token`) is polled for.
/// Called before every park so the reactor re-checks current readiness — this is what
/// makes edge-triggered polling lose no wakeup between an op's `WouldBlock` and its
/// park.
pub(crate) fn reregister_readiness(
    source: &mut impl Source,
    token: Token,
    interest: Interest,
) -> io::Result<()> {
    with_reactor(|reactor| reactor.reregister(source, token, interest))
}

/// Remove `source` from the reactor (on close/drop) so its token stops firing. A
/// no-op if no reactor is active (e.g. a source outliving the scheduler run).
pub(crate) fn deregister_readiness(source: &mut impl Source) {
    REACTOR.with(|r| {
        if let Some(reactor) = r.borrow().as_ref() {
            let _ = reactor.deregister(source);
        }
    });
}

/// Run `f` against the active reactor. Borrowed only in short scopes, never across a
/// `resume` or a park, so a resumed fiber may re-enter freely.
fn with_reactor<R>(f: impl FnOnce(&mut Reactor) -> R) -> R {
    REACTOR.with(|r| f(r.borrow_mut().as_mut().expect("no active reactor")))
}

/// Run the scheduler until every fiber has finished. Seeds `main` as the first
/// fiber, then loops: drain the ready queue (resuming each fiber until it finishes
/// or parks), block the reactor until the nearest wake deadline, and move due
/// fibers back to ready.
///
/// Must be called on a thread registered with the Boehm GC (the process main
/// thread is registered by `GC_init`; tests register explicitly). Not re-entrant.
pub fn run<F: FnOnce() + 'static>(main: F) {
    gc::install_hooks();
    gc::begin_run();

    let already = SCHEDULER.with(|s| s.borrow().is_some());
    assert!(!already, "run() is not re-entrant");
    SCHEDULER.with(|s| *s.borrow_mut() = Some(Scheduler::new()));
    let reactor = Reactor::new().expect("failed to create reactor");
    REACTOR.with(|r| *r.borrow_mut() = Some(reactor));

    spawn_with_stack(SEED_STACK_SIZE, main);

    loop {
        // Pop the next ready fiber AND move it out of the slab in one borrow, so no
        // SCHEDULER borrow is held across `resume` (the fiber may re-enter the
        // scheduler, e.g. call `spawn`).
        while let Some((id, mut fiber)) = with_scheduler(|scheduler| {
            scheduler
                .ready
                .pop_front()
                .map(|id| (id, scheduler.fibers[id].take().unwrap()))
        }) {
            let result = resume_fiber(id, fiber.stack_high, &mut fiber.coroutine);

            match result {
                CoroutineResult::Yield(Park::Sleep(deadline)) => with_scheduler(|scheduler| {
                    scheduler.fibers[id] = Some(fiber);
                    scheduler.timers.push((deadline, id));
                }),
                CoroutineResult::Yield(Park::Readiness(token)) => with_scheduler(|scheduler| {
                    scheduler.fibers[id] = Some(fiber);
                    scheduler.readiness_waiters.insert(token, id);
                }),
                CoroutineResult::Yield(Park::Waiting(address)) => with_scheduler(|scheduler| {
                    scheduler.fibers[id] = Some(fiber);
                    scheduler
                        .address_waiters
                        .entry(address)
                        .or_default()
                        .push(id);
                }),
                CoroutineResult::Yield(Park::CaseAborted) => unreachable!(
                    "only a coroutine run_case_guarded resumes yields this, and only its own \
                     loop ever resumes one — never the ready queue this loop drains"
                ),
                CoroutineResult::Yield(Park::AbortTrapped(..)) => unreachable!(
                    "only a coroutine run_abort_trap_guarded resumes yields this, and only \
                     its own loop ever resumes one — never the ready queue this loop drains"
                ),
                CoroutineResult::Return(()) => {
                    // Unregister the stack range before dropping the fiber, which
                    // unmaps its stack: never leave a range in the GC registry that
                    // points at freed memory.
                    gc::unregister(id);
                    drop(fiber);
                    with_scheduler(|scheduler| {
                        scheduler.fibers[id] = None;
                        scheduler.free.push(id);
                    });
                }
            }
        }

        // Ready queue is empty: either everything finished, or fibers are parked on a timer,
        // a source's readiness, or both. Compute the nearest timer as the poll timeout
        // (`None` = block until a source fires); break only when nothing is parked.
        //
        // `address_waiters` (fibers forcing a deferred or waiting on the stdin gate) is
        // deliberately NOT part of the termination test: a fiber only waits on an address
        // when another fiber will wake it, and that other fiber makes progress by running or
        // by parking on readiness/a timer — never solely on an address itself (a producing
        // read fiber parks on readiness while it holds the stdin gate). So whenever an address
        // waiter exists, `ready`/`timers`/`readiness_waiters` is non-empty too; reaching the
        // break with address waiters left would be a genuine deadlock, and stopping is the
        // right response to that rather than blocking forever.
        let (next_deadline, readiness_parked) = with_scheduler(|scheduler| {
            let next = scheduler.timers.iter().map(|(d, _)| *d).min();
            (next, !scheduler.readiness_waiters.is_empty())
        });
        match (next_deadline, readiness_parked) {
            (None, false) => break, // nothing ready, nothing parked => all done
            (Some(deadline), _) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                wait_and_wake(Some(remaining));
            }
            (None, true) => wait_and_wake(None),
        }

        // Move due timers back to ready.
        with_scheduler(|scheduler| {
            let now = Instant::now();
            let mut i = 0;
            while i < scheduler.timers.len() {
                if scheduler.timers[i].0 <= now {
                    let (_, id) = scheduler.timers.swap_remove(i);
                    scheduler.ready.push_back(id);
                } else {
                    i += 1;
                }
            }
        });
    }

    REACTOR.with(|r| *r.borrow_mut() = None);
    SCHEDULER.with(|s| *s.borrow_mut() = None);
}

/// The C-ABI entry the generated `main` calls to run any program's `^` on this scheduler.
/// `entry` is the generated `__ql_entry` thunk, which has the C `main` signature; its `i32`
/// result is the program's exit code.
///
/// Running `^` as the seed fiber gives every `@` primitive it reaches a fiber to park on.
/// A program that never parks still pays [`run`]'s set-up: a reactor and one fiber stack.
#[unsafe(no_mangle)]
pub extern "C" fn __run_fiber_main(
    entry: extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int,
    argc: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // The seed fiber writes its exit code out through a shared cell. `Rc<Cell<_>>` is
    // `'static` (no borrow of a stack local) and `Clone`, which is all the closure needs:
    // the tier is single-threaded, so no `Send`/synchronization is involved.
    let code = Rc::new(Cell::new(0));
    let code_writer = code.clone();
    run(move || {
        code_writer.set(entry(argc, argv, envp));
    });
    code.get()
}

/// One reactor wait servicing both clocks: block until `timeout` (the nearest sleep
/// deadline) or a source becomes ready, then move every fiber whose token fired back
/// to the ready queue. Tokens with no waiter (already woken, or a stale event) are
/// ignored. Ready tokens are collected before touching the scheduler so no reactor
/// and scheduler borrow are held at once.
fn wait_and_wake(timeout: Option<Duration>) {
    let ready_tokens: Vec<Token> = REACTOR.with(|r| {
        let mut reactor = r.borrow_mut();
        let reactor = reactor.as_mut().expect("no active reactor");
        reactor.wait(timeout);
        reactor.ready_tokens().collect()
    });
    if ready_tokens.is_empty() {
        return;
    }
    with_scheduler(|scheduler| {
        for token in ready_tokens {
            if let Some(id) = scheduler.readiness_waiters.remove(&token) {
                scheduler.ready.push_back(id);
            }
        }
    });
}

fn page_size() -> usize {
    // SAFETY: sysconf with a valid name has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as usize } else { 4096 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::__alloc;
    use crate::test_support::GC_LOCK;
    use std::os::raw::{c_int, c_void};
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[link(name = "gc", kind = "static")]
    unsafe extern "C" {
        fn GC_gcollect();
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }

    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }

    type Job = Box<dyn FnOnce() + Send>;

    /// A single, persistent, Boehm-registered worker thread that every GC-touching
    /// test dispatches its body to. Boehm's stop-the-world uses signals to suspend
    /// registered threads; if collections ran on the ephemeral per-test threads the
    /// harness spawns, Boehm would try to signal threads that have since exited and
    /// abort ("Signals delivery fails constantly"). Funneling all fiber work onto one
    /// long-lived registered thread keeps Boehm's thread set stable (main + worker).
    fn gc_worker() -> &'static mpsc::Sender<Job> {
        static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                // Hooks first (they call GC_allow_register_threads), then register
                // this worker once, for the life of the process.
                gc::install_hooks();
                let mut stack_base = GcStackBase {
                    mem_base: ptr::null_mut(),
                };
                unsafe {
                    GC_get_stack_base(&mut stack_base);
                    GC_register_my_thread(&stack_base);
                }
                for job in receiver {
                    job();
                }
            });
            sender
        })
    }

    /// Run `f` on the persistent GC worker, serialized against every other
    /// GC-touching test via `GC_LOCK`, and block until it completes.
    fn on_gc_thread<F: FnOnce() + Send + 'static>(f: F) {
        let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (done_sender, done_receiver) = mpsc::channel();
        gc_worker()
            .send(Box::new(move || {
                f();
                let _ = done_sender.send(());
            }))
            .unwrap();
        done_receiver.recv().unwrap();
    }

    fn collect() {
        unsafe { GC_gcollect() };
    }

    /// Fill a fresh GC allocation of `len` bytes with `byte` and return the pointer.
    fn alloc_filled(len: usize, byte: u8) -> *mut u8 {
        let p = __alloc(len as i64) as *mut u8;
        assert!(!p.is_null());
        unsafe { ptr::write_bytes(p, byte, len) };
        p
    }

    fn all_bytes(p: *mut u8, len: usize, byte: u8) -> bool {
        (0..len).all(|i| unsafe { *p.add(i) } == byte)
    }

    #[test]
    fn sleep_wakes_in_deadline_order() {
        static ORDER: Mutex<Vec<u32>> = Mutex::new(Vec::new());
        ORDER.lock().unwrap().clear();

        on_gc_thread(|| {
            run(|| {
                // Spawn out of deadline order; assert they complete in deadline order.
                spawn(|| {
                    sleep(Duration::from_millis(60));
                    ORDER.lock().unwrap().push(60);
                });
                spawn(|| {
                    sleep(Duration::from_millis(20));
                    ORDER.lock().unwrap().push(20);
                });
                spawn(|| {
                    sleep(Duration::from_millis(40));
                    ORDER.lock().unwrap().push(40);
                });
                sleep(Duration::from_millis(10));
                ORDER.lock().unwrap().push(10);
            });
        });

        assert_eq!(*ORDER.lock().unwrap(), vec![10, 20, 40, 60]);
    }

    #[test]
    fn parked_fiber_stack_roots_survive_collection() {
        // Proves (a): a parked fiber's stack roots are pushed by the callback, so a
        // collection driven while it sleeps does not free its data.
        const N: usize = 32;
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                // Fiber A: allocate, keep the ONLY references on its own stack, sleep
                // long enough for B to force a collection, then verify intact.
                spawn(|| {
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        *slot = alloc_filled(LEN, (i as u8).wrapping_add(1));
                    }
                    let held = std::hint::black_box(held);
                    sleep(Duration::from_millis(40));
                    // Churn the heap after the collection to reuse any wrongly-freed
                    // space, then confirm every held object is byte-for-byte intact.
                    for _ in 0..64 {
                        std::hint::black_box(alloc_filled(LEN, 0xEE));
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        if all_bytes(p, LEN, (i as u8).wrapping_add(1)) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });
                // Fiber B: wake first and force a collection while A is parked.
                spawn(|| {
                    sleep(Duration::from_millis(10));
                    collect();
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }

    #[test]
    fn running_fiber_stack_roots_survive_collection() {
        // Proves (b): a collection triggered while executing ON a fiber stack scans
        // the correct range (via GC_set_stackbottom), so live roots survive.
        const N: usize = 64;
        const LEN: usize = 128;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        *slot = alloc_filled(LEN, (i as u8).wrapping_add(1));
                    }
                    let held = std::hint::black_box(held);
                    // Collect while running on the fiber stack.
                    collect();
                    for _ in 0..128 {
                        std::hint::black_box(alloc_filled(LEN, 0xEE));
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        if all_bytes(p, LEN, (i as u8).wrapping_add(1)) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }

    #[test]
    fn abort_trap_active_reflects_nesting_depth() {
        extern "C" fn returns(_environment: *mut c_void) -> u8 {
            0
        }
        extern "C" fn aborts(_environment: *mut c_void) -> u8 {
            crate::report::fail_at(ptr::null(), 500, "trapped for a unit test", 101)
        }

        on_gc_thread(|| {
            run(|| {
                assert!(!abort_trap_active(), "no trap active outside one");

                let outcome = run_abort_trap_guarded(returns, ptr::null_mut());
                assert!(outcome.is_none(), "a returning lambda does not abort");
                assert!(!abort_trap_active(), "the trap ends once it has returned");

                let outcome = run_abort_trap_guarded(aborts, ptr::null_mut());
                let (exit_code, report) = outcome.expect("the lambda aborted");
                assert_eq!(exit_code, 101);
                assert!(report.contains("trapped for a unit test"));
                assert!(
                    !abort_trap_active(),
                    "the trap ends once it has caught the abort"
                );
            });
        });
    }

    #[test]
    fn a_trap_inside_a_trap_catches_its_own_abort_and_keeps_the_reports_apart() {
        extern "C" fn inner_aborts(_environment: *mut c_void) -> u8 {
            crate::report::fail_at(ptr::null(), 500, "inner", 101)
        }
        extern "C" fn outer(_environment: *mut c_void) -> u8 {
            let inner = run_abort_trap_guarded(inner_aborts, ptr::null_mut());
            assert!(inner.is_some(), "the inner trap must catch its own abort");
            assert!(abort_trap_active(), "the outer trap is still in progress");
            crate::report::fail_at(ptr::null(), 500, "outer", 101)
        }

        on_gc_thread(|| {
            run(|| {
                let outcome = run_abort_trap_guarded(outer, ptr::null_mut());
                let (_, report) = outcome.expect("the outer lambda aborted");
                assert!(report.contains("outer"));
                assert!(
                    !report.contains("inner"),
                    "the inner trap's report stays with the inner trap: {report}"
                );
            });
        });
    }

    #[test]
    fn a_trap_inside_a_case_inside_the_seed_fiber_scans_correctly() {
        // Three fibers deep (seed -> case -> trap) with nothing spawned: proves the
        // trap's own nested fiber is covered by the same "stack of running fibers"
        // bookkeeping (`crate::gc`) a case's already is, by surviving a collection
        // triggered while the trap fiber is the one executing.
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        extern "C" fn trap_body(_environment: *mut c_void) -> u8 {
            let held = alloc_filled(LEN, 0x42);
            let held = std::hint::black_box(held);
            collect();
            for _ in 0..64 {
                std::hint::black_box(alloc_filled(LEN, 0xEE));
            }
            if all_bytes(held, LEN, 0x42) {
                VERIFIED.store(1, Ordering::SeqCst);
            }
            0
        }
        extern "C" fn case_body(_environment: *mut c_void) -> u8 {
            run_abort_trap_guarded(trap_body, ptr::null_mut());
            0
        }

        on_gc_thread(|| {
            run(|| {
                run_case_guarded(case_body, ptr::null_mut());
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), 1);
    }
}
