// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Fiber-stack GC integration for the scheduler.
//!
//! The runtime uses the Boehm conservative collector, which only knows how to scan
//! the stack of the OS thread it is called from (from that thread's recorded stack
//! base down to the current SP). corosensei fibers run on their OWN stacks, so two
//! separate things would otherwise go wrong and either free live data or scan a
//! garbage address range — this is the recipe Crystal's `gc/boehm.cr` uses.
//!
//! (a) PARKED fibers: a suspended fiber's stack holds live GC roots (locals, and the
//!     callee-saved registers corosensei spilled there when it suspended) that Boehm
//!     never scans on its own. We install a `GC_push_other_roots` callback that, on
//!     every collection, pushes each live-but-not-running fiber's usable stack range
//!     with `GC_push_all_eager`. Pushing the whole usable region (not just the used
//!     part) is deliberately conservative: over-scanning committed stack is safe for
//!     a conservative collector, and it needs no per-suspend stack-pointer capture.
//!
//! (b) The RUNNING fiber: while executing ON a fiber stack, Boehm's automatic scan
//!     would pair the MAIN thread's recorded stack base with the current SP (which is
//!     on a different stack) — a meaningless, huge range. On every switch INTO a
//!     fiber we set Boehm's stack base to that fiber's base with `GC_set_stackbottom`,
//!     so the automatic scan covers exactly the fiber's used stack; on switch back we
//!     restore whichever base was current before (the main thread's, unless a test
//!     case's guard resumed this fiber's own coroutine from inside another fiber, in
//!     which case that outer fiber's). Only the innermost fiber is excluded from (a);
//!     every fiber beneath it in such a nested resume is scanned by (a) like a parked
//!     one, since the automatic scan can only ever cover the innermost.
//!
//! ## Running fibers on more than one OS thread (spike)
//!
//! Two changes make (a) and (b) hold even when several OS threads each drive their own
//! scheduler concurrently, fibers pinned to whichever thread spawned them:
//!
//! **Switch ordering.** [`enter_fiber`]/[`leave_fiber`] run from the FIBER's own code
//! (`crate::scheduler::new_fiber`'s entry, and `suspend_on` around every park), not from
//! the resumer's side wrapping an opaque `coroutine.resume()` call — the resumer cannot
//! observe "has the jump landed yet", only the fiber's own code, running after the jump,
//! can. [`enter_fiber`] then points the automatic scan at this fiber's stack
//! (`GC_set_stackbottom`) BEFORE excluding it from the parked-fiber scan, so the fiber is
//! never covered by neither: while stackbottom is being updated, ranges-scan still covers
//! it; once ranges-scan drops it, stackbottom already does. [`leave_fiber`] runs the two
//! steps in the mirrored order — re-included in the parked scan before stackbottom is
//! handed back to the outer context — for the same reason, called as the last fiber-side
//! act before suspending (or returning). The remaining instant — the raw stack-pointer
//! switch inside `corosensei`'s resume/suspend itself — is NOT closed by any ordering of
//! Rust statements on either side of it, because that switch is opaque: nothing here runs
//! while it is in flight, so `GC_set_stackbottom`'s target and the true stack pointer are
//! briefly for two different stacks. Whether that is harmless depends on which two
//! addresses happen to be involved — depending on where the allocator happened to place
//! the outer context's stack relative to the fiber's own (heap-allocated) one, the
//! resulting bottom/current-SP pair can come out the "wrong" way round, which Boehm's
//! automatic per-thread scan walks as a real, usually mapped, range instead of skipping
//! it as empty. **This spike's stress test found exactly that**: under sustained
//! collections against many fibers per thread, the process reliably segfaults inside
//! Boehm's own marker (`GC_mark_from`), seconds into a run — see the PR body for the
//! reproduction and the verdict. These two changes are necessary (they close the one gap
//! a Rust-level reordering CAN close) but not sufficient; closing the remaining one needs
//! either disabling collection for the switch itself (throughput cost, and not free to
//! place correctly around an opaque `corosensei` call — see the PR body) or never relying
//! on `GC_set_stackbottom` for a fiber's stack at all (registering it as a permanent GC
//! root instead, the way `crate::mem::PinnedPointer` already does for other roots).
//!
//! **Per-thread scan state.** What was one global `Mutex<GcState>` is now one immutable
//! [`ThreadState`] snapshot per thread, held behind a plain [`AtomicPtr`] in a fixed-size
//! [`REGISTRY`] slot the owning thread alone ever swaps (`publish`, below) — one pointer
//! write per state change, no lock. [`push_fiber_roots`] (Boehm's world-stopped callback,
//! running on whichever thread is driving a collection) walks every registered slot
//! without ever taking a lock a thread it just stopped could be holding: the collector's
//! stop-the-world guarantees every OTHER registered thread is frozen for the whole call,
//! so the only thread that could ever free a slot's old snapshot (the slot's own owner,
//! right after swapping in a new one) is provably not doing so concurrently with this
//! read — either it published the new snapshot before being stopped (this callback then
//! reads the new one, valid) or it has not yet started freeing the old one (still valid).
//!
//! Any pre-existing `GC_push_other_roots` callback is chained, not clobbered, via a
//! process-wide [`OnceLock`] set once, before the collector's hook is installed, and read
//! (never blocking, once set) by every later collection. The `#[no_mangle]` GC intrinsics
//! (`__gc_init`, the allocation binding) live in [`crate::mem`]; this module holds only
//! the scheduler-side scanning integration.

use crate::mem::__gc_init;
use crate::stack_overflow;
use std::cell::Cell;
use std::os::raw::c_void;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

#[repr(C)]
struct GcStackBase {
    mem_base: *mut c_void,
}

type GcPushOtherRootsProc = unsafe extern "C" fn();

#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_set_push_other_roots(f: GcPushOtherRootsProc);
    fn GC_get_push_other_roots() -> Option<GcPushOtherRootsProc>;
    fn GC_push_all_eager(bottom: *mut c_void, top: *mut c_void);
    fn GC_set_stackbottom(h: *mut c_void, sb: *const GcStackBase) -> *mut c_void;
    fn GC_get_my_stackbottom(sb: *mut GcStackBase) -> *mut c_void;
    fn GC_allow_register_threads();
}

/// An immutable, whole-state snapshot for one thread: its live fiber ranges, the fibers
/// currently executing (outermost first, see the module doc's point (b)), and its own base
/// stack address. Published as a unit — never mutated in place — so a reader on another
/// thread (`push_fiber_roots`, only ever running with this thread already stopped) sees a
/// internally-consistent view with no lock.
struct ThreadState {
    /// Per-fiber usable stack range `[low, high)`, indexed by fiber id (`None` when the
    /// slot is free). Shared via `Arc` rather than cloned whole on every publish: `running`
    /// (below) changes on every single fiber switch, `ranges` only on register/unregister,
    /// so bumping a refcount for the common case is far cheaper than copying the whole
    /// vector every switch.
    ranges: std::sync::Arc<Vec<Option<(usize, usize)>>>,
    running: Vec<usize>,
    main_base: usize,
}

impl ThreadState {
    fn fresh(main_base: usize) -> Self {
        ThreadState {
            ranges: std::sync::Arc::new(Vec::new()),
            running: Vec::new(),
            main_base,
        }
    }
}

/// Upper bound on how many OS threads may ever call [`begin_run`]/[`register`]/etc. — a
/// fixed-size array needs one. Generous for a spike (a handful of scheduler threads plus
/// whatever the test suite's shared GC worker and the blocking pool ever register).
const MAX_THREADS: usize = 256;

/// One slot per registered thread, written only by its owner (see the module doc).
/// `AtomicPtr` rather than a `Mutex<Box<ThreadState>>`: `push_fiber_roots` must read every
/// slot with no lock a stopped thread could hold.
static REGISTRY: [AtomicPtr<ThreadState>; MAX_THREADS] = {
    // The repeated `const` below is a single array-repeat expression, not a shared
    // instance every use rebinds to — each array slot gets its own, independently
    // initialized `AtomicPtr`, so clippy's usual worry about a duplicated interior-mutable
    // const does not apply.
    #[allow(clippy::declare_interior_mutable_const)]
    const NULL: AtomicPtr<ThreadState> = AtomicPtr::new(ptr::null_mut());
    [NULL; MAX_THREADS]
};

/// How many slots in [`REGISTRY`] are in use (an upper bound on valid indices — a slot at
/// or past this count is guaranteed unclaimed; one already claimed but not yet published
/// is `null`, handled by [`push_fiber_roots`]).
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's own registry slot, assigned once on first use.
    static MY_SLOT: Cell<Option<usize>> = const { Cell::new(None) };

    /// The GC-internal thread handle [`GC_get_my_stackbottom`] returned for this thread,
    /// passed to every later `GC_set_stackbottom` call instead of `NULL` — the collector's
    /// own doc for that function says a `NULL` handle resolves "the current thread" through
    /// a path that expects the caller to already hold the GC lock, which nothing here does
    /// on every fiber switch; a handle fetched once and reused needs no such lookup.
    static MY_GC_HANDLE: Cell<*mut c_void> = const { Cell::new(ptr::null_mut()) };
}

/// The previously installed `GC_push_other_roots` callback, if any, captured once during
/// [`install_hooks`] before ours replaces it. A `OnceLock` rather than the state behind a
/// lock: reading it (every collection, from [`push_fiber_roots`]) never blocks once it is
/// set, and it is set — synchronously, before `GC_set_push_other_roots` makes our callback
/// reachable at all — long before any collection could possibly read it.
static PREVIOUS_CALLBACK: OnceLock<Option<GcPushOtherRootsProc>> = OnceLock::new();

/// This thread's slot index in [`REGISTRY`], claiming and initializing one on first call.
fn my_slot() -> usize {
    MY_SLOT.with(|cell| {
        if let Some(slot) = cell.get() {
            return slot;
        }
        let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        assert!(
            slot < MAX_THREADS,
            "more threads registered with the GC fiber scanner than MAX_THREADS"
        );
        let initial = Box::new(ThreadState::fresh(0));
        REGISTRY[slot].store(Box::into_raw(initial), Ordering::Release);
        cell.set(Some(slot));
        slot
    })
}

/// Read this thread's current snapshot. Only ever called by the thread that owns the
/// slot (so the pointer it loads is always the latest one IT published, never freed out
/// from under it — see the module doc).
fn my_state() -> &'static ThreadState {
    let ptr = REGISTRY[my_slot()].load(Ordering::Acquire);
    // SAFETY: `ptr` was published by this very call's own thread (the only writer of its
    // slot) and is never freed while still the current pointer.
    unsafe { &*ptr }
}

/// Publish a new snapshot for this thread, built by `f` from the current one, and free the
/// snapshot it replaces. One atomic pointer swap; see the module doc for why the free that
/// follows can never race a reader.
fn publish(f: impl FnOnce(&ThreadState) -> ThreadState) {
    let slot = my_slot();
    let old_ptr = REGISTRY[slot].load(Ordering::Acquire);
    // SAFETY: this thread's own most recent publish (or its initial one from `my_slot`).
    let old = unsafe { &*old_ptr };
    let new_ptr = Box::into_raw(Box::new(f(old)));
    REGISTRY[slot].store(new_ptr, Ordering::Release);
    // SAFETY: `old_ptr` was this thread's own previous publish, and only this thread ever
    // frees its own slot's pointers; a reader on another thread only ever runs with this
    // thread already stopped by the collector, never concurrently with this free.
    unsafe { drop(Box::from_raw(old_ptr)) };
}

/// Boehm calls this during a collection (world stopped, every OTHER registered thread
/// frozen). Walks every registered thread's snapshot, pushing each live fiber range not
/// currently the innermost-running one, then chains to any previously-installed callback.
extern "C" fn push_fiber_roots() {
    let used = NEXT_SLOT.load(Ordering::Acquire).min(MAX_THREADS);
    for slot in &REGISTRY[..used] {
        let ptr = slot.load(Ordering::Acquire);
        if ptr.is_null() {
            // A slot index was claimed (NEXT_SLOT bumped) but its owner has not yet
            // stored its first snapshot — it therefore has no fibers registered yet.
            continue;
        }
        // SAFETY: see `publish`'s own safety note — this thread is frozen for the whole
        // call, so it cannot be freeing this exact pointer right now.
        let state = unsafe { &*ptr };
        for (id, range) in state.ranges.iter().enumerate() {
            if let Some((low, high)) = *range
                && state.running.last() != Some(&id)
            {
                // SAFETY: [low, high) is that fiber's committed, readable stack (above
                // the guard page); pushing it is what keeps its roots alive.
                // GC_push_all_eager neither allocates nor re-enters us.
                unsafe { GC_push_all_eager(low as *mut c_void, high as *mut c_void) };
            }
        }
    }
    // SAFETY: `PREVIOUS_CALLBACK` is set (to whatever Boehm handed back) before our
    // callback is ever installed, so it is always initialized by the time this runs.
    if let Some(previous) = PREVIOUS_CALLBACK.get().copied().flatten() {
        unsafe { previous() };
    }
}

/// Give this thread the guard-page stack-overflow handler, then run the rest — GC init,
/// foreign-thread registration, chaining in the push-roots callback — once for the
/// process. `stack_overflow::install` runs on every call, ahead of the once-only gate:
/// its alternate signal stack is a per-THREAD attribute, so every thread that ever calls
/// this (one for a compiled program; one per scheduler the test suite, and this spike's
/// stress test, run concurrently) needs its own, while the rest of this setup need only
/// happen the first time.
pub(crate) fn install_hooks() {
    stack_overflow::install();

    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        __gc_init();
        // SAFETY: all one-time GC configuration on an initialized collector.
        unsafe {
            GC_allow_register_threads();
            let previous = GC_get_push_other_roots();
            // `PREVIOUS_CALLBACK` is set before the collector's hook can possibly reach
            // our callback, so `push_fiber_roots` never observes it uninitialized.
            let _ = PREVIOUS_CALLBACK.set(previous);
            GC_set_push_other_roots(push_fiber_roots);
        }
    });
}

/// Reset per-run state and record the current thread's own base stack address and GC
/// thread handle. Called at the start of every scheduler run (both are thread-specific).
pub(crate) fn begin_run() {
    let mut stack_base = GcStackBase {
        mem_base: ptr::null_mut(),
    };
    // SAFETY: `GC_get_my_stackbottom` fills `stack_base` with the current thread's base
    // and hands back its GC-internal handle; the thread must already be registered with
    // the collector (the scheduler's own doc requires this of every caller of `run`).
    let handle = unsafe { GC_get_my_stackbottom(&mut stack_base) };
    MY_GC_HANDLE.with(|h| h.set(handle));
    let main_base = stack_base.mem_base as usize;
    publish(|_old| ThreadState::fresh(main_base));
}

pub(crate) fn register(id: usize, low: usize, high: usize) {
    publish(|old| {
        let mut ranges = (*old.ranges).clone();
        if id >= ranges.len() {
            ranges.resize(id + 1, None);
        }
        ranges[id] = Some((low, high));
        ThreadState {
            ranges: std::sync::Arc::new(ranges),
            running: old.running.clone(),
            main_base: old.main_base,
        }
    });
}

pub(crate) fn unregister(id: usize) {
    publish(|old| {
        let mut ranges = (*old.ranges).clone();
        if id < ranges.len() {
            ranges[id] = None;
        }
        ThreadState {
            ranges: std::sync::Arc::new(ranges),
            running: old.running.clone(),
            main_base: old.main_base,
        }
    });
}

/// Point Boehm's automatic stack scan at the stack whose cold end (base) is `base` — the
/// running fiber's, or the main thread's/an outer fiber's on the way back out. Uses this
/// thread's own GC handle (see [`MY_GC_HANDLE`]) rather than `NULL`.
fn set_stackbottom(base: usize) {
    let stack_base = GcStackBase {
        mem_base: base as *mut c_void,
    };
    let handle = MY_GC_HANDLE.with(|h| h.get());
    // SAFETY: `stack_base` is read (copied) synchronously by the call; `handle` is this
    // same thread's own GC-internal handle, fetched once in `begin_run`.
    unsafe { GC_set_stackbottom(handle, &stack_base) };
}

/// Called from the FIBER's own code, right after it lands (either the first statement its
/// body ever runs, or the first statement after a `suspend()` call returns) — never from
/// the resumer's side, which cannot observe when the jump has actually landed. Points the
/// automatic scan at this fiber's stack, THEN excludes it from the parked-fiber scan, in
/// that order: while stackbottom is being updated the fiber is still ranges-covered, and by
/// the time it is excluded stackbottom has already taken over. See the module doc.
pub(crate) fn enter_fiber(id: usize) {
    let high = my_state().ranges[id]
        .expect("a fiber must be registered before it is entered")
        .1;
    set_stackbottom(high);
    publish(|old| {
        let mut running = old.running.clone();
        running.push(id);
        ThreadState {
            ranges: old.ranges.clone(),
            running,
            main_base: old.main_base,
        }
    });
}

/// Called from the FIBER's own code, as the very last act before it suspends (or before it
/// returns, ending the fiber) — mirrors [`enter_fiber`]'s ordering: re-included in the
/// parked-fiber scan FIRST, stackbottom handed back to the outer context second, so the
/// fiber is never covered by neither. See the module doc.
pub(crate) fn leave_fiber() {
    let mut outer_base = 0usize;
    publish(|old| {
        let mut running = old.running.clone();
        running.pop();
        outer_base = match running.last() {
            Some(&id) => old.ranges[id].expect("a running fiber stays registered").1,
            None => old.main_base,
        };
        ThreadState {
            ranges: old.ranges.clone(),
            running,
            main_base: old.main_base,
        }
    });
    set_stackbottom(outer_base);
}
