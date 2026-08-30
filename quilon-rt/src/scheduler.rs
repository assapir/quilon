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
use std::os::raw::{c_char, c_int};
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

    fn alloc_slot(&mut self, fiber: Fiber) -> usize {
        if let Some(id) = self.free.pop() {
            self.fibers[id] = Some(fiber);
            id
        } else {
            self.fibers.push(Some(fiber));
            self.fibers.len() - 1
        }
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
}

/// Run `f` against the active scheduler. A short borrow only — never held across a
/// `resume`, so a resumed fiber may re-enter the scheduler freely.
fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    SCHEDULER.with(|s| f(s.borrow_mut().as_mut().expect("no active scheduler")))
}

/// Spawn `f` as a new fiber and enqueue it, with a stack of `stack_size` bytes.
fn spawn_with_stack<F: FnOnce() + 'static>(stack_size: usize, f: F) {
    let stack = DefaultStack::new(stack_size).expect("failed to allocate fiber stack");
    let base = stack.base().get();
    let limit = stack.limit().get();
    // Usable region is [limit + guard_page, base); the guard page sits at the low
    // end of the mapping. Scanning from just above it never faults on PROT_NONE.
    let stack_low = limit + page_size();
    let stack_high = base;

    let coroutine: FiberCoroutine = Coroutine::with_stack(stack, move |yielder, ()| {
        CURRENT_YIELDER.with(|c| c.set(yielder as *const FiberYielder));
        f();
    });

    SCHEDULER.with(|s| {
        let mut slot = s.borrow_mut();
        let scheduler = slot
            .as_mut()
            .expect("spawn() called with no active scheduler");
        let id = scheduler.alloc_slot(Fiber {
            coroutine,
            stack_high,
        });
        scheduler.ready.push_back(id);
        gc::register(id, stack_low, stack_high);
    });
}

/// Spawn `f` as a child fiber, with the standard [`FIBER_STACK_SIZE`] stack. Call it from
/// within a running fiber; [`run`] seeds the program's own fiber directly, at
/// [`SEED_STACK_SIZE`]. Panics if no scheduler is active.
pub fn spawn<F: FnOnce() + 'static>(f: F) {
    spawn_with_stack(FIBER_STACK_SIZE, f);
}

/// Park the current fiber until `duration` elapses, yielding to the scheduler. Must
/// be called from within a fiber (panics otherwise).
pub fn sleep(duration: Duration) {
    let yielder = CURRENT_YIELDER.get();
    assert!(!yielder.is_null(), "sleep() called outside a fiber");
    let deadline = Instant::now() + duration;
    // SAFETY: `yielder` points at the live `Yielder` for this fiber, valid for the
    // whole fiber body (it is a parameter of the corosensei closure we are inside).
    unsafe { (*yielder).suspend(Park::Sleep(deadline)) };
    // Resumed: sibling fibers ran and overwrote the shared cell; restore ours so
    // later code on this fiber still finds its yielder.
    CURRENT_YIELDER.set(yielder);
}

/// Park the current fiber until the reactor reports `token` ready, yielding to the
/// scheduler. The caller ([`crate::net`]) must have (re)registered the source for the
/// interest it needs *before* calling this, so the readiness that wakes it is the one
/// it is waiting for. Must be called from within a fiber (panics otherwise).
pub(crate) fn park_on_readiness(token: Token) {
    let yielder = CURRENT_YIELDER.get();
    assert!(
        !yielder.is_null(),
        "park_on_readiness() called outside a fiber"
    );
    // SAFETY: `yielder` points at the live `Yielder` for this fiber (see `sleep`).
    unsafe { (*yielder).suspend(Park::Readiness(token)) };
    CURRENT_YIELDER.set(yielder);
}

/// Park the current fiber until another fiber wakes `address`. The caller re-checks its own
/// condition after every wake (a wake is an invitation to look, never a guarantee), so a
/// spurious or shared wake simply re-parks. Must be called from within a fiber (panics
/// otherwise).
pub(crate) fn park_on_address(address: usize) {
    let yielder = CURRENT_YIELDER.get();
    assert!(
        !yielder.is_null(),
        "park_on_address() called outside a fiber"
    );
    // SAFETY: `yielder` points at the live `Yielder` for this fiber (see `sleep`).
    unsafe { (*yielder).suspend(Park::Waiting(address)) };
    CURRENT_YIELDER.set(yielder);
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
            gc::enter_fiber(id, fiber.stack_high);
            let result = fiber.coroutine.resume(());
            gc::leave_fiber();

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
}
