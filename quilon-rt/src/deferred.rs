// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Deferred values — the runtime half of Quilon's colorless implicit futures.
//!
//! A value-returning `@` primitive does not park the calling fiber and hand back bytes;
//! it *launches* the IO on a background fiber and returns a **deferred value** immediately. The
//! value flows through the program as an ordinary one (a `Text`, here) and is *forced*
//! only when a strict primitive is about to read its concrete bytes. Forcing parks the
//! current fiber until the producing fiber has stored the result, then reads it — memoized,
//! so a second force is O(1).
//!
//! [`__read_launch`] backs `@read` (read one line from stdin): it allocates a `Deferred`,
//! spawns a reader fiber that parks on stdin readiness and fills the cell, and returns the
//! deferred [`QnSlice`] representation. [`__force_text`] is the force: park-until-ready,
//! then return the stored bytes.
//!
//! Representation (hybrid, per the concurrency design): a `Text` is a `{ ptr, i64 }`
//! `QnSlice`. A *ready* `Text` carries its byte length (`>= 0`) in the second field; a
//! *deferred* `Text` carries [`DEFERRED_SENTINEL`] (`-1`) there and the deferred pointer in
//! the first — a real byte length is never negative, so the two are unambiguous. The code
//! generator forces exactly at the strict-use sites the deferred-taint pass marks, and only
//! for values that pass can be deferred; pure code never sees a sentinel and pays nothing.
//!
//! GC: the deferred cell is GC-allocated so its stored `data` pointer keeps the result bytes
//! alive; the reader fiber holds the cell on its (GC-scanned) stack from launch until it
//! returns, and the forcing fiber holds it across the park — so a collection at any point in
//! the deferred lifetime finds it. (A parked fiber's stack is scanned by the collector's
//! push-roots callback; `scheduler`/`net` tests prove that scanning directly.)
//!
//! Stdin is a single serial stream, so all reads are serialized through a **gate**: a reader
//! acquires the gate before it touches fd 0 and releases it when done, so at most one reader
//! ever owns the descriptor and the shared line buffer at a time. Two concurrent `@readStdin`
//! calls therefore read consecutive lines in launch order rather than racing the fd.

use crate::mem::{__alloc, QnSlice, alloc_text};
use crate::reactor::ReactorWaker;
use crate::report::{QnSite, RUNTIME_EXIT_CODE, codes, fail_at};
use crate::scheduler::{
    deregister_readiness, park_on_readiness, register_helper_waker, register_readiness,
    reregister_readiness, spawn, spawn_placed,
};
use mio::unix::SourceFd;
use mio::{Interest, Token};
use std::io;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Mutex;

/// The second (`i64`) field of a deferred `Text`'s `QnSlice`: a real byte length is never
/// negative, so `-1` unambiguously flags "the first field is a deferred pointer, not data".
/// The code generator's force check compares against this exact value.
pub const DEFERRED_SENTINEL: i64 = -1;

/// A Quilon `Result` whose payload is a `Text`, in the code generator's canonical layout
/// (`{ i8 tag, {ptr,i64} slot }`): a `Text` is itself a `{ptr,i64}`, so it fills the slot
/// directly with no boxing. `#[repr(C)]` puts the tag at offset 0 and the slot at offset 8 —
/// the same offsets LLVM emits for `{ i8, {ptr,i64} }` — so the two representations agree by
/// construction. The tags are the code generator's built-in Result discriminants.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QnResult {
    pub(crate) tag: i8,
    pub(crate) slot: QnSlice,
}

/// The `Ok` discriminant — the code generator's built-in Result tag for the success variant.
pub(crate) const RESULT_OK_TAG: i8 = 0;
/// The `NotOk` discriminant — the code generator's built-in Result tag for the failure variant.
pub(crate) const RESULT_NOTOK_TAG: i8 = 1;
/// The tag a DEFERRED `Result` carries in place of `Ok`/`NotOk`, with the deferred cell pointer
/// stashed in its slot's `data` field; the code generator's force check compares against it.
/// Distinct from every real discriminant, so a ready `Result` is never mistaken for a deferred
/// one.
pub const DEFERRED_RESULT_TAG: i8 = -1;

impl QnResult {
    /// A ready `Ok(text)` carrying `bytes` as its `Text` payload.
    pub(crate) fn ok(bytes: &[u8]) -> QnResult {
        QnResult {
            tag: RESULT_OK_TAG,
            slot: alloc_text(bytes),
        }
    }

    /// A ready `NotOk(message)` carrying `message` as its `Text` payload.
    pub(crate) fn not_ok(message: &str) -> QnResult {
        QnResult {
            tag: RESULT_NOTOK_TAG,
            slot: alloc_text(message.as_bytes()),
        }
    }

    /// A ready `Ok(number)` carrying `number` as its `Num` payload — the slot packed the same
    /// way the code generator's `pack_result_payload` packs a Num (`{ null, bitcast(f64) }`),
    /// so a Rust-built `Ok(Num)` reads back identically to one `.qn` source constructs.
    pub(crate) fn ok_num(number: f64) -> QnResult {
        QnResult {
            tag: RESULT_OK_TAG,
            slot: QnSlice {
                data: ptr::null(),
                len: number.to_bits() as i64,
            },
        }
    }
}

/// A deferred value's lifecycle. Carrying the resolved value INSIDE `Ready` makes
/// "ready but value absent" unrepresentable — a whole class of bug a bool/flag plus a
/// separate value field would allow. (Distinct from [`DEFERRED_SENTINEL`], which is the
/// C-ABI representation tag, not a lifecycle state.)
enum DeferredState<T> {
    /// The producer has not finished yet.
    Pending,
    /// The producer stored its result.
    Ready(T),
    /// The producer hit a fail-loud condition instead — the fully rendered report it would
    /// have written straight to stderr, held here instead so the enclosing block's join can
    /// report it once every sibling launch has settled (see `crate::launch_scope`).
    Faulted(String),
}

/// The reactor tokens of every fiber — on any worker — currently parked waiting on a
/// condition, alongside the [`ReactorWaker`] that wakes each one's own worker. Woken through
/// the same mechanism a blocking-call's completion already uses (`crate::blocking`), so a
/// cross-worker wake costs no more than that.
///
/// Carries no lock of its own: it is always a field of a structure some OTHER `Mutex`
/// already guards (`DeferredInner` below; the stdin gate's own state), and THAT lock is what
/// makes [`register`](Self::register)-then-park atomic with respect to a concurrent
/// [`wake_all`](Self::wake_all) — a wake landing between a waiter's own condition check and
/// its registration is impossible only because both run while the same lock is held.
#[derive(Default)]
struct Waiters(Vec<(ReactorWaker, Token)>);

impl Waiters {
    /// Register the calling fiber as a waiter, returning the token it must
    /// [`park_on_readiness`] on once the caller has released the lock that guards the
    /// condition being waited on.
    fn register(&mut self) -> Token {
        let (token, waker) = register_helper_waker();
        self.0.push((waker, token));
        token
    }

    /// Wake every registered waiter, on whichever worker each one is parked.
    fn wake_all(&mut self) {
        for (waker, token) in self.0.drain(..) {
            waker.complete(token);
        }
    }
}

struct DeferredInner<T> {
    state: DeferredState<T>,
    waiters: Waiters,
}

/// A deferred value's cell — the generic core every value-returning `@` primitive shares.
/// GC-allocated so any GC pointer inside a `Ready` value stays scannable. The producer that
/// resolves it and a fiber forcing it may run on DIFFERENT workers (placement puts a
/// freestanding launch's producer on whichever worker has the shortest run queue, which need
/// not be the launching fiber's own), so the state is `Mutex`-guarded rather than the plain
/// field a single cooperative thread could get away with — one lock per force/resolve, never
/// held across a park. Opaque to the code generator, which only ever carries the cell
/// pointer around and hands it back to a `force` intrinsic.
pub(crate) struct Deferred<T> {
    inner: Mutex<DeferredInner<T>>,
}

/// Build a fresh, `Pending` deferred cell on the GC heap — the part [`launch_here`] and
/// [`launch_placed`] share before they differ on where the producer actually runs.
fn new_deferred<T>() -> *mut Deferred<T> {
    let size = std::mem::size_of::<Deferred<T>>();
    let cell = __alloc(size as i64) as *mut Deferred<T>;
    // The cell must be a fresh, GC-zeroed allocation, never a live one we would clobber.
    // `GC_malloc` zeroes, so a fresh cell reads as all-zero bytes; reading the not-yet-typed
    // memory as `u8` is well-defined (no invalid bit patterns), unlike reading it as `T`.
    // Debug-only; `__alloc` aborts rather than handing back null, so the read is safe.
    debug_assert!(
        unsafe { std::slice::from_raw_parts(cell as *const u8, size) }
            .iter()
            .all(|&byte| byte == 0),
        "initializing a Deferred cell that is not fresh"
    );
    // SAFETY: `__alloc` returned a fresh, aligned cell of exactly this size; initialize it
    // before anyone can observe it (`ptr::write`, since the memory is uninitialized as `T`).
    unsafe {
        ptr::write(
            cell,
            Deferred {
                inner: Mutex::new(DeferredInner {
                    state: DeferredState::Pending,
                    waiters: Waiters::default(),
                }),
            },
        );
    }
    cell
}

/// The producer body every launch shares, whichever worker ends up running it: run
/// `producer` guarded against a fail-loud exit ([`crate::scheduler::run_fault_guarded`], so
/// an IO error settles the cell as [`DeferredState::Faulted`] instead of exiting mid-launch),
/// then store the outcome and wake every waiter — on whichever worker each is parked.
///
/// A cell is resolved exactly once; a second resolve would clobber live data and signal a
/// real cross-worker race. Guarded in debug builds.
fn resolve_body<T: 'static>(
    cell_address: usize,
    producer: impl FnOnce() -> T + 'static,
) -> impl FnOnce() + 'static {
    move || {
        let outcome = crate::scheduler::run_fault_guarded(producer);
        // SAFETY: `cell_address` is `new_deferred`'s own return value, pinned (see
        // `launch_here`/`launch_placed`) until this resolves.
        let cell = cell_address as *mut Deferred<T>;
        let mut inner = unsafe { &(*cell).inner }
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        debug_assert!(
            matches!(inner.state, DeferredState::Pending),
            "resolving an already-resolved Deferred"
        );
        inner.state = match outcome {
            Ok(value) => DeferredState::Ready(value),
            Err(report) => DeferredState::Faulted(report),
        };
        inner.waiters.wake_all();
    }
}

/// Register `cell`'s join with whatever `< >` block's launch scope is open right now (see
/// `crate::launch_scope`), so that block settles it — whether or not its value is ever
/// forced — before returning.
///
/// A bound-but-unread launch's own alloca may be dead-store-eliminated once the compiled
/// program never reads it again (the whole point of joining it is that nothing else has
/// to), which would leave nothing scanned pointing at `cell` between the producer finishing
/// and the join reading it — so the cell is [pinned](crate::mem::PinnedPointer) from here
/// until the registered join thunk has read its final state.
fn register_join<T: 'static>(cell: *mut Deferred<T>) {
    let pin = crate::mem::PinnedPointer::new(cell as *mut c_void);
    crate::launch_scope::register(move || {
        let outcome = unsafe { settle(cell) };
        drop(pin); // the cell's final state is read; the runtime no longer needs it pinned
        outcome
    });
}

/// Launch `producer` on the CALLING fiber's own worker and return its deferred cell
/// immediately — eager launch: the producer runs whether or not the result is ever forced.
/// For a launch whose producer is tied to a resource already scoped to a specific worker: an
/// accepted connection's own reads (the connection's socket is registered with its handler's
/// worker's reactor, so a read on it must run there too), and `net.@tcpServe`'s accept loop
/// itself, held to the worker `net.@tcpServe` was called from (see `net::server`'s own doc).
pub(crate) fn launch_here<T: Send + 'static>(
    producer: impl FnOnce() -> T + 'static,
) -> *mut Deferred<T> {
    let cell = new_deferred::<T>();
    spawn(resolve_body(cell as usize, producer));
    register_join(cell);
    cell
}

/// Launch `producer` on whichever worker has the shortest run queue right now — every
/// freestanding value-returning `@` primitive the deferral pass registers a launch scope
/// around (`@readStdin`, `@tcpRequest`), which has no worker of its own to stay on. `producer`
/// must be `Send`: it is built on the calling thread but never runs there — see
/// `scheduler::spawn_placed`'s own doc for why that is sound despite `Coroutine` itself
/// being `!Send`.
pub(crate) fn launch_placed<T: Send + 'static>(
    producer: impl FnOnce() -> T + Send + 'static,
) -> *mut Deferred<T> {
    let cell = new_deferred::<T>();
    spawn_placed(resolve_body(cell as usize, producer));
    register_join(cell);
    cell
}

/// Force a deferred value: park the current fiber until `cell` is resolved, then return the
/// value (memoized — a second force is O(1)). `T: Copy` so the value is read out without
/// disturbing the cell that other forces still read.
///
/// A `Faulted` cell never yields a value: reporting it is the enclosing block's join's job
/// (see `crate::launch_scope`), so a strict force that finds one there reaches that same
/// join early instead — every sibling launch of the block still settles first, exactly as
/// the block's own close would settle it.
///
/// # Safety
/// `cell` is a live deferred for the whole force (the taint pass keeps it reachable to here).
pub(crate) unsafe fn force<T: Copy>(cell: *mut Deferred<T>) -> T {
    loop {
        // SAFETY: `cell` is a live deferred (see the contract).
        let inner_mutex = unsafe { &(*cell).inner };
        let token = {
            let mut inner = inner_mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &inner.state {
                // `T: Copy`, so this reads the value out without disturbing the cell that
                // other forces still read (memoized).
                DeferredState::Ready(value) => return *value,
                DeferredState::Faulted(report) => {
                    let report = report.clone();
                    drop(inner);
                    crate::launch_scope::fault_current_fiber(report)
                }
                // Register before releasing the lock — see `Waiters`'s own doc for why that
                // makes a concurrent resolve's wake impossible to miss.
                DeferredState::Pending => inner.register(),
            }
        };
        park_on_readiness(token);
        // Re-loop and re-check: a wake is an invitation to look, never a guarantee.
    }
}

/// Park until `cell` settles — either outcome, without reading a value out — what a launch's
/// join needs: it runs to completion whether or not anything ever forces it. `None` if it
/// resolved to a value, `Some(report)` (the fully rendered fault text) if it faulted.
///
/// `pub(crate)` (rather than the join thunk's own private helper) so a launch whose RETURN
/// VALUE is never deferred — `net.@tcpServe`'s accept loop, registered directly with
/// `launch_scope` rather than through [`launch_deferred_text`]/[`launch_deferred_result`] —
/// can wait out its own background work the same way: `Server.kill` calls this on the accept
/// loop's cell to know it has actually stopped before returning.
///
/// # Safety
/// `cell` is a live deferred for the whole wait (the registering `launch_here`/`launch_placed`
/// call's own join thunk is one caller; `net`'s server-kill path, holding the pointer that
/// call returned, is the other — neither captures anything that outlives the cell).
pub(crate) unsafe fn settle<T>(cell: *mut Deferred<T>) -> Option<String> {
    loop {
        // SAFETY: `cell` is a live deferred (see the contract).
        let inner_mutex = unsafe { &(*cell).inner };
        let token = {
            let mut inner = inner_mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &inner.state {
                DeferredState::Ready(_) => return None,
                DeferredState::Faulted(report) => return Some(report.clone()),
                DeferredState::Pending => inner.register(),
            }
        };
        park_on_readiness(token);
    }
}

impl<T> DeferredInner<T> {
    fn register(&mut self) -> Token {
        self.waiters.register()
    }
}

/// Launch a `Text`-producing IO on a background fiber, placed on the shortest-queue worker,
/// and return its DEFERRED `Text` representation (`{ deferred, -1 }`) immediately — the
/// C-ABI wrapper over [`launch_placed`] that every FREESTANDING value-returning `@`
/// primitive shares (`@readStdin`, `@tcpRequest`), so none re-copies the sentinel-tagging.
/// The result threads through the program as an ordinary `Text`; the code generator forces
/// it (via [`__force_text`]) at its strict-use site.
pub(crate) fn launch_deferred_text(producer: impl FnOnce() -> QnSlice + Send + 'static) -> QnSlice {
    let cell = launch_placed(producer);
    QnSlice {
        data: cell as *const c_void,
        len: DEFERRED_SENTINEL,
    }
}

/// Like [`launch_deferred_text`], but placed on the CALLING fiber's own worker
/// ([`launch_here`]) rather than the shortest queue — `Connection.@read()`'s own launch: the
/// connection's socket is already registered with a specific worker's reactor (its
/// handler's own, set once at accept time), so the read must run there too, not wherever
/// happens to have the shortest queue right now.
pub(crate) fn launch_deferred_text_here(producer: impl FnOnce() -> QnSlice + 'static) -> QnSlice {
    let cell = launch_here(producer);
    QnSlice {
        data: cell as *const c_void,
        len: DEFERRED_SENTINEL,
    }
}

/// Launch a `Result`-producing IO on a background fiber, placed on the shortest-queue
/// worker, and return its DEFERRED `Result` representation immediately — the C-ABI wrapper
/// over [`launch_placed`] for a value-returning `@` primitive whose result is a `Result`
/// (`@tcpRequest`), so its Ok/NotOk wrapping and force plumbing are shared, not re-copied.
/// The deferred representation is a `Result` value tagged [`DEFERRED_RESULT_TAG`] with the
/// deferred cell in its slot's `data` field; the result threads through the program as an
/// ordinary `Result` and the code generator forces it (via [`__force_result`]) at the strict
/// use that reads it.
pub(crate) fn launch_deferred_result(
    producer: impl FnOnce() -> QnResult + Send + 'static,
) -> QnResult {
    let cell = launch_placed(producer);
    QnResult {
        tag: DEFERRED_RESULT_TAG,
        slot: QnSlice {
            data: cell as *const c_void,
            len: 0,
        },
    }
}

/// Force a deferred `Result`, writing the resolved `{ tag, slot }` into `out`: the
/// per-representation C-ABI wrapper over the generic `force`. A `Result` is 24 bytes, which the
/// C ABI returns via a hidden pointer rather than in registers (unlike the 16-byte `Text`), so
/// the value is passed back through an out-pointer the code generator supplies — no aggregate
/// return crosses the FFI boundary. Only the code generator calls this, and only after its force
/// check saw [`DEFERRED_RESULT_TAG`], so `deferred_ptr` is always a live `Result` deferred.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QnResult`]; `deferred_ptr` is the slot `data` of a
/// deferred `Result` produced by `launch_deferred_result` and is still reachable (the taint pass
/// keeps it live to here).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_result(out: *mut QnResult, deferred_ptr: *const c_void) {
    // SAFETY: per the contract, this is a live `Result` deferred (a `Deferred<QnResult>`).
    let value = unsafe { force(deferred_ptr as *mut Deferred<QnResult>) };
    // SAFETY: `out` is writable storage for one `QnResult` (the code generator's alloca).
    unsafe { *out = value };
}

/// `@readStdin()`: launch a background read of one line from stdin and return the deferred
/// `Text` immediately (the calling fiber does not park here). A THIN wrapper over the shared
/// `launch_deferred_text`, with a stdin-specific producer. `site` is the call's own location,
/// used to frame a fault report; it may be null if unknown.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a [`QnSite`] constant that outlives the program (the code
/// generator emits one read-only global per call site).
#[unsafe(no_mangle)]
pub extern "C" fn __read_launch(site: *const QnSite) -> QnSlice {
    // The site is a read-only constant in the module, so it outlives the launched read;
    // crossed as a `usize` because a raw pointer alone is not `Send` (placement may run the
    // producer on another worker), the same way `net::client`'s own address/request
    // pointers cross a `spawn` boundary.
    let site = site as usize;
    // Taken HERE, synchronously, on the launching fiber, before the producer is even built —
    // not inside the producer's own body, which only starts running once placement has
    // handed it to some worker and that worker gets around to it. See `take_stdin_ticket`'s
    // own doc for why ticket order (fixed now) rather than gate-arrival order (decided later,
    // by racing OS thread scheduling) is what "launch order" actually requires.
    let ticket = take_stdin_ticket();
    launch_deferred_text(move || read_stdin_text(site as *const QnSite, ticket))
}

/// Force a deferred `Text`: the per-representation C-ABI wrapper over the generic `force`.
/// Only the code generator calls this, and only after its force check saw [`DEFERRED_SENTINEL`],
/// so `deferred_ptr` is always a live `Text` deferred.
///
/// # Safety contract (upheld by the compiler)
/// `deferred_ptr` is a pointer previously returned in the first field of a deferred
/// `__read_launch` result and is still reachable (the taint pass keeps it live to here).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_text(deferred_ptr: *const c_void) -> QnSlice {
    // SAFETY: per the contract, this is a live `Text` deferred (a `Deferred<QnSlice>`).
    unsafe { force(deferred_ptr as *mut Deferred<QnSlice>) }
}

/// The `@readStdin` producer: read one line from stdin as a `Text`, serialized on the stdin
/// gate so concurrent reads take consecutive lines rather than racing fd 0. Yields the empty
/// `Text` at end-of-input; a genuine IO error faults at the launch site (fail-loud).
fn read_stdin_text(site: *const QnSite, ticket: u64) -> QnSlice {
    acquire_stdin(ticket);
    let read = read_stdin_line();
    release_stdin();
    match read {
        Ok(bytes) => alloc_text(&bytes),
        Err(error) => fail_read(site, &error),
    }
}

/// Report a fatal stdin read error at the `@readStdin` launch site, then terminate the
/// process (fail-loud). A genuine IO error on stdin is neither EOF nor `WouldBlock`.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QnSite`].
fn fail_read(site: *const QnSite, error: &io::Error) -> ! {
    fail_at(
        site,
        codes::READ_FAILED,
        &format!("core.io.@readStdin failed: {error}"),
        RUNTIME_EXIT_CODE,
    )
}

/// fd 0 is one descriptor for the whole PROCESS, not one per worker — a gate that serialized
/// reads only among fibers on the SAME worker would let a fiber on one worker and a fiber on
/// another both believe they hold it at once and race the same descriptor. One process-wide
/// `Mutex`, not a `Worker` field or a thread-local, covers every worker alike.
///
/// A TICKET lock, not a plain busy flag: two concurrent `@readStdin` calls must read
/// consecutive lines in LAUNCH order, and launch order is decided once, at the moment
/// `__read_launch` runs — before its producer fiber even exists, let alone which worker
/// placement hands it to or when that worker's OS thread actually gets scheduled to run it
/// for the first time. A plain "whoever reaches the gate first, or whoever a broadcast wake
/// happens to let run first, wins" gate only preserved launch order by accident under the
/// original single-threaded scheduler (one ready queue, so whichever fiber's turn came first
/// in program order was also, unavoidably, whichever one the scheduler ran first) — across
/// real OS threads racing to reach the gate, or racing a broadcast wake to re-acquire it,
/// that accident stops holding. Handing out a ticket at launch time and serving strictly in
/// ticket order removes the race entirely: order is fixed before any concurrency starts.
struct StdinGate {
    next_ticket: u64,
    /// The one ticket currently allowed through.
    serving: u64,
    leftover: Vec<u8>,
    waiters: Waiters,
}

static STDIN_GATE: Mutex<StdinGate> = Mutex::new(StdinGate {
    next_ticket: 0,
    serving: 0,
    leftover: Vec::new(),
    waiters: Waiters(Vec::new()),
});

const STDIN_FD: i32 = 0;

/// Hand out the next stdin-read ticket, synchronously, on the calling (launching) fiber —
/// call this from `__read_launch` itself, before the producer that will eventually
/// [`acquire_stdin`] with it is even built. See [`StdinGate`]'s own doc for why ticket order
/// must be fixed this early.
fn take_stdin_ticket() -> u64 {
    let mut gate = STDIN_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let ticket = gate.next_ticket;
    gate.next_ticket += 1;
    ticket
}

/// Wait until `ticket` is being served — i.e., every earlier ticket has already released the
/// gate — on any worker.
fn acquire_stdin(ticket: u64) {
    loop {
        let token = {
            let mut gate = STDIN_GATE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if gate.serving == ticket {
                return;
            }
            gate.waiters.register()
        };
        park_on_readiness(token);
        // Re-loop and re-check: a broadcast wake tells every waiting ticket to look, not
        // which one's turn it actually is — only the matching ticket proceeds; every other
        // one re-registers and parks again.
    }
}

/// Release stdin: advance to the next ticket and wake every waiter to re-check (only the
/// one now being served actually proceeds).
fn release_stdin() {
    let mut gate = STDIN_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    gate.serving += 1;
    gate.waiters.wake_all();
}

/// Read one line from stdin (fd 0), the source `@readStdin` reads. Thin wrapper over
/// [`read_line_from`]: it takes the persistent leftover buffer out of the gate while reading
/// (so the lock is never held across the fiber park) and stores what remains back
/// afterwards, so successive reads continue the same stream. The caller holds the stdin
/// gate, so there is exactly one reader in here at a time, on any worker.
fn read_stdin_line() -> io::Result<Vec<u8>> {
    let mut buffer = std::mem::take(
        &mut STDIN_GATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .leftover,
    );
    let result = read_line_from(STDIN_FD, &mut buffer);
    STDIN_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .leftover = buffer;
    result
}

/// Read one line from `fd` into `buffer`, parking the fiber (via [`read_once`]) until a
/// newline arrives or the stream ends. Returns the line WITHOUT its trailing newline (a
/// trailing `\r` is dropped too). At end-of-input with nothing buffered, returns an empty
/// `Vec` — the documented end-of-input value (`@read` yields an empty `Text` there). Bytes
/// past the newline stay in `buffer` for the next call.
fn read_line_from(fd: i32, buffer: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    if let Some(line) = take_line(buffer) {
        return Ok(line);
    }
    set_nonblocking(fd);
    let mut chunk = [0u8; 1024];
    loop {
        let count = read_once(fd, &mut chunk)?;
        if count == 0 {
            // EOF: hand back whatever is buffered (an unterminated final line), or empty.
            return Ok(std::mem::take(buffer));
        }
        buffer.extend_from_slice(&chunk[..count]);
        if let Some(line) = take_line(buffer) {
            return Ok(line);
        }
    }
}

/// Read once from `fd` into `buffer`, parking the fiber on reactor readiness (lazily
/// registered) through any number of `WouldBlock`s, and retrying an interrupted call
/// (`EINTR`), until data or end-of-input is available. Returns the byte count read (`0` at
/// EOF). The shared primitive behind every fiber-parking descriptor read: [`read_line_from`]
/// (stdin, accumulating until a newline) and `@streamFile` (`crate::io`, chunk-buffered) each
/// build their own loop over it.
///
/// The reactor registration is LAZY: it reads first and only registers `fd` (and parks) on the
/// first `WouldBlock`. So a source that is ready right away — piped data already buffered, a
/// regular file (always), or a non-pollable fd like `/dev/null` that returns data/EOF at once —
/// never touches `epoll`, which rejects such fds. Only a genuinely-not-ready pollable source
/// (an empty pipe/tty) is registered and parked on. Registering after a `WouldBlock` loses no
/// wakeup: adding an already-ready fd to the poll reports it immediately.
pub(crate) fn read_once(fd: i32, buffer: &mut [u8]) -> io::Result<usize> {
    let mut source = SourceFd(&fd);
    let mut token: Option<Token> = None;
    let result = loop {
        // SAFETY: `read(2)` into a valid, owned buffer of `buffer.len()` bytes.
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut c_void, buffer.len()) };
        if count >= 0 {
            break Ok(count as usize);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock => {
                let active = match token {
                    Some(active) => {
                        match reregister_readiness(&mut source, active, Interest::READABLE) {
                            Ok(()) => active,
                            Err(error) => break Err(error),
                        }
                    }
                    None => match register_readiness(&mut source, Interest::READABLE) {
                        Ok(active) => {
                            token = Some(active);
                            active
                        }
                        Err(error) => break Err(error),
                    },
                };
                park_on_readiness(active);
            }
            io::ErrorKind::Interrupted => {}
            _ => break Err(error),
        }
    };
    if token.is_some() {
        deregister_readiness(&mut source);
    }
    result
}

/// If `buffer` holds a complete line (up to and including a `\n`), remove and return it
/// without the newline (and without a preceding `\r`); otherwise `None`.
fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline = buffer.iter().position(|&byte| byte == b'\n')?;
    let mut line: Vec<u8> = buffer.drain(..=newline).collect();
    line.pop(); // drop the '\n'
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Put `fd` into non-blocking mode so a `read` on an empty pipe returns `WouldBlock` and
/// parks the fiber, rather than blocking the single OS thread. Failure is tolerable — a
/// still-blocking read simply blocks (functionally fine when nothing else is runnable).
pub(crate) fn set_nonblocking(fd: i32) {
    // SAFETY: `fcntl` on a descriptor; a bad fd just returns an error we ignore.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc_test_harness::on_gc_thread;
    use crate::scheduler::{run, sleep, spawn};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A `pipe(2)` pair, returned as `(read_end, write_end)`.
    fn make_pipe() -> (i32, i32) {
        let mut fds = [0i32; 2];
        // SAFETY: `pipe` fills a 2-element array with the two descriptors.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe() failed");
        (fds[0], fds[1])
    }

    /// Like [`__read_launch`] but reading one line from an arbitrary `fd` (a pipe), so a test
    /// can drive the producer with a controllable writer. Exercises the generic [`launch`] core
    /// with a pipe-reading producer and returns the deferred `{deferred, -1}` representation.
    fn launch_read_from_fd(fd: i32) -> QnSlice {
        launch_deferred_text(move || {
            let mut buffer = Vec::new();
            let bytes = read_line_from(fd, &mut buffer).expect("pipe read");
            alloc_text(&bytes)
        })
    }

    #[test]
    fn stdin_gate_serializes_readers() {
        // Two fibers contend for the stdin gate. The gate must keep at most one inside the
        // acquire/release section at a time (so concurrent `@readStdin` launches never race
        // fd 0), even though each yields (sleeps) while holding it.
        static CONCURRENT: AtomicUsize = AtomicUsize::new(0);
        static MAX_CONCURRENT: AtomicUsize = AtomicUsize::new(0);
        CONCURRENT.store(0, Ordering::SeqCst);
        MAX_CONCURRENT.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                for _ in 0..2 {
                    let ticket = take_stdin_ticket();
                    spawn(move || {
                        acquire_stdin(ticket);
                        let now = CONCURRENT.fetch_add(1, Ordering::SeqCst) + 1;
                        MAX_CONCURRENT.fetch_max(now, Ordering::SeqCst);
                        // Yield while holding the gate; a second reader must wait, not enter.
                        sleep(Duration::from_millis(10));
                        CONCURRENT.fetch_sub(1, Ordering::SeqCst);
                        release_stdin();
                    });
                }
            });
        });

        assert_eq!(
            MAX_CONCURRENT.load(Ordering::SeqCst),
            1,
            "the stdin gate let two readers hold stdin at once"
        );
    }

    #[test]
    fn take_line_splits_on_newline_and_keeps_remainder() {
        let mut buffer = b"first\r\nsecond".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"first".to_vec()));
        assert_eq!(buffer, b"second");
        // No newline yet: nothing to take.
        assert_eq!(take_line(&mut buffer), None);
        assert_eq!(buffer, b"second");
    }

    #[test]
    fn deferred_read_launches_and_forces_the_line() {
        // Proves the whole value-returning path: `@read` launches a background reader that
        // must PARK on stdin readiness (the writer is delayed), a separate fiber FORCES the
        // deferred value (parking on the deferred), and the read line flows through.
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        GOT.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let deferred_ptr = deferred.data;

                spawn(move || {
                    let forced = __force_text(deferred_ptr);
                    let bytes = crate::text::byte_slice(forced.data as *const u8, forced.len);
                    *GOT.lock().unwrap() = bytes.to_vec();
                });

                // Delay the write so the reader is already parked on pipe readiness when it
                // arrives — the park path, not a lucky already-ready read, is what runs.
                spawn(move || {
                    sleep(Duration::from_millis(20));
                    let message = b"hello world\n";
                    unsafe {
                        libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
                    }
                });
            });
        });

        assert_eq!(&*GOT.lock().unwrap(), b"hello world");
    }

    #[test]
    fn force_is_memoized_after_ready() {
        // Forcing the same deferred value twice returns the same bytes, and the second force
        // never parks (the cell is already READY).
        static FIRST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static SECOND: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        FIRST.lock().unwrap().clear();
        SECOND.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            let message = b"line\n";
            // Write before the run so the value is ready without any park.
            unsafe {
                libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
            }
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let deferred_ptr = deferred.data;
                spawn(move || {
                    let a = __force_text(deferred_ptr);
                    let b = __force_text(deferred_ptr);
                    let read =
                        |s: QnSlice| crate::text::byte_slice(s.data as *const u8, s.len).to_vec();
                    *FIRST.lock().unwrap() = read(a);
                    *SECOND.lock().unwrap() = read(b);
                });
            });
        });

        assert_eq!(&*FIRST.lock().unwrap(), b"line");
        assert_eq!(&*SECOND.lock().unwrap(), b"line");
    }
}
