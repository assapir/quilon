// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Fiber-stack GC integration for the scheduler — round 2 of the multi-thread spike
//! (design C, superseding round 1's scheme entirely; see the PR body for both rounds'
//! numbers and verdicts).
//!
//! The problem is unchanged from round 1's doc: the Boehm conservative collector only
//! knows how to scan the stack of the OS thread it is called from, but `corosensei`
//! fibers run on their own stacks. Round 1 answered this by moving Boehm's own idea of
//! "this thread's stack" back and forth between the native stack and whichever fiber was
//! running (`GC_set_stackbottom`), backstopped by a hand-pushed scan of every PARKED
//! fiber's range. That left one gap neither change could close: the raw stack-pointer
//! switch inside `corosensei`'s `resume`/`suspend` is opaque, so for an instant
//! `GC_set_stackbottom`'s target and the true stack pointer are for two different
//! stacks — and whether a collection landing exactly then is harmless depends on the
//! *relative addresses* of those two stacks. It reliably was not: the round-1 stress test
//! segfaulted inside Boehm's own marker within seconds.
//!
//! Design C removes that gap by never touching `GC_set_stackbottom` for a fiber at all:
//!
//! 1. **Every fiber's stack is a permanent GC root for its whole lifetime.**
//!    `register` calls `GC_add_roots` when a fiber's stack is allocated; `unregister`
//!    calls `GC_remove_roots` right before it is freed. Once added, a fiber's stack is
//!    scanned on every collection unconditionally — running, parked, or (harmlessly) not
//!    yet ever resumed — the same as any other static root Boehm already knows about
//!    (`.data`/`.bss`, or a JIT'd program's globals via `crate::mem::__gc_add_root`).
//!    There is no more per-fiber "is it the one running" state to track at all.
//! 2. **The native thread is declared blocked while a fiber runs.** `do_blocking` wraps
//!    the scheduler's `coroutine.resume()` call in `GC_do_blocking`: Boehm records this
//!    thread's stack pointer at the point of that call and scans only up to there,
//!    treating everything deeper (the whole span the fiber actually runs in, including
//!    the raw switch itself) as outside its scan — which is fine, because that memory is
//!    the FIBER's stack now, already covered unconditionally by (1), not the native
//!    thread's. There is no instant where the wrong thing is scanned: the fiber's stack
//!    is always covered by (1); the native thread's own stack is scanned up to exactly
//!    where it stopped touching anything GC-relevant, for the whole time it is blocked.
//! 3. **A blocked thread must not touch the GC heap without saying so.** `with_gc_active`
//!    wraps `alloc_via` (`crate::mem`'s sole `GC_malloc`/`GC_malloc_atomic` call site) in
//!    `GC_call_with_gc_active`, which temporarily re-activates normal scanning for the
//!    call — covering the allocating function's own frame precisely, rather than relying
//!    on it having nothing live there. Calling it when the thread was never blocked in the
//!    first place (native code before the scheduler starts, or the blocking-pool threads
//!    in `crate::blocking`, which never run fibers) is a cheap, correct no-op per bdwgc's
//!    own source (`pthread_support.c`'s `GC_call_with_gc_active`: `if (!me->thread_blocked)
//!    { ...; return fn(client_data); }`) — so every allocation goes through it
//!    unconditionally, one code path, rather than branching on whether a fiber happens to
//!    be running.
//!
//! Both `GC_do_blocking` and `GC_call_with_gc_active` take the collector's global
//! allocation lock on entry and exit (see `pthread_support.c`); `GC_add_roots`/
//! `GC_remove_roots` do too, plus a linear or hashed scan of every existing root (bdwgc's
//! own doc calls the latter "wizards only"). Design C trades round 1's lock-free,
//! per-switch bookkeeping for these locked, per-switch AND per-allocation calls — see the
//! PR body's performance table for what that costs. `GC_add_roots` also has a hard,
//! process-wide ceiling (`MAX_ROOT_SETS`, 2048 in this build's configuration) on how many
//! *live* root ranges can exist at once — every concurrently-alive fiber (across every
//! thread) counts against it, which a production M:N scheduler would need to keep well
//! under.

use crate::mem::{__gc_init, GcThread, register_thread};
use crate::stack_overflow;
use std::cell::{Cell, RefCell};
use std::os::raw::{c_int, c_void};
use std::ptr;

/// `GC_fn_type` (`gc.h`): the callback signature both [`GC_do_blocking`] and
/// [`GC_call_with_gc_active`] take.
type GcFnType = extern "C" fn(*mut c_void) -> *mut c_void;

#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_add_roots(low_address: *mut c_void, high_address_plus_1: *mut c_void);
    fn GC_remove_roots(low_address: *mut c_void, high_address_plus_1: *mut c_void);
    fn GC_do_blocking(fn_: GcFnType, client_data: *mut c_void) -> *mut c_void;
    fn GC_call_with_gc_active(fn_: GcFnType, client_data: *mut c_void) -> *mut c_void;
    fn GC_allow_register_threads();
    fn GC_thread_is_registered() -> c_int;
}

/// Give this thread the guard-page stack-overflow handler, then run the rest — GC init and
/// foreign-thread registration — once for the process. `stack_overflow::install` runs on
/// every call, ahead of the once-only gate: its alternate signal stack is a per-THREAD
/// attribute, so every thread that ever calls this needs its own, while the rest of this
/// setup need only happen the first time. Unlike round 1, there is no scan callback to
/// install here at all: every fiber's stack is a plain, permanent root (see the module
/// doc), so Boehm needs nothing extra chained into its own root-scanning pass.
pub(crate) fn install_hooks() {
    stack_overflow::install();

    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        __gc_init();
        // SAFETY: one-time GC configuration on an initialized collector.
        unsafe { GC_allow_register_threads() };
    });
}

/// Register a fiber's usable stack range `[low, high)` as a permanent GC root for as long
/// as the fiber exists. Called once, at spawn — never toggled per switch, unlike round 1's
/// `enter_fiber`/`leave_fiber`, because a permanent root is scanned every collection
/// regardless of whether the fiber is currently running or parked.
pub(crate) fn register(low: usize, high: usize) {
    // SAFETY: `[low, high)` is a fiber's own committed, readable stack (above its guard
    // page), valid until the matching `unregister` call.
    unsafe { GC_add_roots(low as *mut c_void, high as *mut c_void) };
}

/// Undo [`register`]: remove `[low, high)` from the permanent root set. Must be called
/// before the stack backing it is freed or reused (never leave a root pointing at memory
/// that is no longer this fiber's).
pub(crate) fn unregister(low: usize, high: usize) {
    // SAFETY: `[low, high)` is exactly the range a matching `register` call added.
    unsafe { GC_remove_roots(low as *mut c_void, high as *mut c_void) };
}

/// Run `f` and hand back its result, through one of the two `GC_fn_type` C callbacks
/// [`do_blocking`]/[`with_gc_active`] wrap — shared so their trampoline (boxing `f`'s
/// result out through a C `void*` callback) exists exactly once.
fn call_through<F: FnOnce() -> R, R>(
    gc_call: unsafe extern "C" fn(GcFnType, *mut c_void) -> *mut c_void,
    f: F,
) -> R {
    struct Passed<F, R> {
        f: Option<F>,
        result: Option<R>,
    }
    let mut passed = Passed {
        f: Some(f),
        result: None,
    };

    extern "C" fn trampoline<F: FnOnce() -> R, R>(data: *mut c_void) -> *mut c_void {
        // SAFETY: `data` is `&mut Passed<F, R>`, alive for the whole call below — it is a
        // stack local of `call_through`, which does not return until `gc_call` does, and
        // `gc_call` invokes this trampoline exactly once, synchronously, before returning.
        let passed = unsafe { &mut *(data as *mut Passed<F, R>) };
        let f = passed
            .f
            .take()
            .expect("a GC_fn_type callback runs exactly once");
        passed.result = Some(f());
        ptr::null_mut()
    }

    // SAFETY: `trampoline::<F, R>` matches `GcFnType`; `passed` outlives the call (it is
    // this function's own local, not returned to until `gc_call` has finished with it).
    unsafe {
        gc_call(
            trampoline::<F, R>,
            &mut passed as *mut Passed<F, R> as *mut c_void,
        )
    };
    passed
        .result
        .take()
        .expect("the trampoline always runs before GC_do_blocking/GC_call_with_gc_active return")
}

thread_local! {
    /// How many nested [`do_blocking`] regions this thread is currently inside — nested
    /// because `crate::scheduler::resume_fiber` is not just the ready queue's own call:
    /// `run_case_guarded`/`run_fault_guarded` resume a coroutine from INSIDE another
    /// fiber's own execution (already itself resumed through this same function), so a
    /// naive wrap-every-call would call `GC_do_blocking` recursively on one thread — which
    /// bdwgc does not support (`GC_do_blocking_inner` is not written to be re-entered; see
    /// `pthread_support.c`'s own comment on it). Only the outermost call actually wraps.
    static DO_BLOCKING_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Run `f` — a plain closure, not a `GC_fn_type` — with this thread declared "blocked" for
/// the call's whole duration: Boehm scans this thread's own stack only up to the point of
/// this call, not into whatever `f` does deeper (see the module doc). The one caller is
/// `crate::scheduler::resume_fiber`, wrapping exactly the raw `coroutine.resume()` call —
/// including a NESTED one, which runs `f` directly instead of re-entering `GC_do_blocking`
/// (see [`DO_BLOCKING_DEPTH`]'s own doc); the outermost region already covers it.
pub(crate) fn do_blocking<R>(f: impl FnOnce() -> R) -> R {
    let depth = DO_BLOCKING_DEPTH.get();
    if depth > 0 {
        return f();
    }
    DO_BLOCKING_DEPTH.set(depth + 1);
    let result = call_through(GC_do_blocking, f);
    DO_BLOCKING_DEPTH.set(depth);
    result
}

thread_local! {
    /// Whether this thread's registration status has been settled — either it was already
    /// registered by someone else (the scheduler's own thread, or a JIT host that called
    /// `crate::register_thread` itself before running any Quilon code) or [`ensure_registered`]
    /// registered it and is holding the guard, below. Checked once per thread, not once per
    /// allocation: `GC_thread_is_registered` still takes the collector's lock.
    static REGISTRATION_SETTLED: Cell<bool> = const { Cell::new(false) };

    /// This thread's own registration, held for the rest of its life if [`ensure_registered`]
    /// is the one that performed it — its `Drop` (run via thread-local destruction when this
    /// thread exits, e.g. a plain test's ephemeral thread) unregisters it. `None` when some
    /// other, earlier call already registered this thread; there must be exactly one owner.
    static SELF_REGISTERED: RefCell<Option<GcThread>> = const { RefCell::new(None) };
}

/// `alloc_via` (via [`with_gc_active`]) is reachable from any thread that ever allocates a
/// `Text`/array/record — not only a scheduler thread, whose own contract already requires
/// pre-registration, but a plain unit test's thread, which has never registered with the
/// collector at all. `GC_call_with_gc_active` assumes the caller already has, and
/// dereferences a null lookup result if not — so this settles it first, exactly once per
/// thread, before ever making that call.
fn ensure_registered() {
    if REGISTRATION_SETTLED.replace(true) {
        return;
    }
    __gc_init();
    // SAFETY: a plain query, no preconditions beyond the collector being initialized
    // (`__gc_init` above, idempotent, guarantees that).
    if unsafe { GC_thread_is_registered() } == 0 {
        SELF_REGISTERED.with(|cell| *cell.borrow_mut() = Some(register_thread()));
    }
}

/// Run `f` — a plain closure, not a `GC_fn_type` — with this thread declared "active" (safe
/// to call any GC function or touch the heap) for the call's duration, even if it is
/// currently inside a [`do_blocking`] region. Safe, and a cheap no-op wrapper, to call
/// unconditionally outside one too — see the module doc.
pub(crate) fn with_gc_active<R>(f: impl FnOnce() -> R) -> R {
    ensure_registered();
    call_through(GC_call_with_gc_active, f)
}
