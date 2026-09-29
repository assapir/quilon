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
//! ## Running fibers on more than one OS thread
//!
//! Two changes make (a) and (b) hold when several OS threads each drive their own
//! `crate::worker::Worker` concurrently, fibers pinned to whichever thread spawned them:
//!
//! **Switch ordering.** `enter_fiber`/`leave_fiber` (below) run from the FIBER's own code
//! (`crate::scheduler::new_fiber`'s entry, and `suspend_on` around every park), not from
//! the resumer's side wrapping an opaque `coroutine.resume()` call — the resumer cannot
//! observe "has the jump landed yet", only the fiber's own code, running after the jump,
//! can. `enter_fiber` points the automatic scan at this fiber's stack
//! (`GC_set_stackbottom`) BEFORE excluding it from the parked-fiber scan, so the fiber is
//! never covered by neither: while stackbottom is being updated, ranges-scan still covers
//! it; once ranges-scan drops it, stackbottom already does. `leave_fiber` runs the two
//! steps in the mirrored order — re-included in the parked scan before stackbottom is
//! handed back to the outer context — for the same reason, called as the last fiber-side
//! act before suspending (or returning). Neither ordering alone closes the raw
//! stack-pointer switch inside `corosensei`'s `resume`/`suspend` itself: nothing runs
//! while it is in flight, so `GC_set_stackbottom`'s target and the true stack pointer are
//! briefly for two different stacks, and a collection landing exactly then can walk one
//! of them as a real, largely-unmapped range depending on where the allocator happened to
//! place it relative to the other — `switch_disable`/`switch_enable`, below, close
//! that remaining instant.
//!
//! **Per-thread scan state.** What would otherwise be one global, lock-guarded registry
//! is one immutable `ThreadState` snapshot per thread, held behind a plain `AtomicPtr`
//! in a fixed-size `REGISTRY` slot the owning thread alone ever swaps (`publish`) —
//! one pointer write per state change, no lock. `push_fiber_roots` (Boehm's
//! world-stopped callback, running on whichever thread is driving a collection) walks
//! every registered slot without ever taking a lock a thread it just stopped could be
//! holding: the collector's stop-the-world guarantees every OTHER registered thread is
//! frozen for the whole call, so the only thread that could ever free a slot's old
//! snapshot (the slot's own owner, right after swapping in a new one) is provably not
//! doing so concurrently with this read — either it published the new snapshot before
//! being stopped (this callback then reads the new one, valid) or it has not yet started
//! freeing the old one (still valid).
//!
//! Any pre-existing `GC_push_other_roots` callback is chained, not clobbered, via a
//! process-wide [`OnceLock`] set once, before the collector's hook is installed, and read
//! (never blocking, once set) by every later collection. The `#[no_mangle]` GC intrinsics
//! (`__gc_init`, the allocation binding) live in [`crate::mem`]; this module holds only
//! the scheduler-side scanning integration.
//!
//! ## The switch guard: holding the collector off across every switch
//!
//! Per-thread scan state and fiber-side switch ordering close the gap between "excluded
//! from the parked scan" and "the automatic scan is actually pointed at this fiber" — but
//! not the raw stack-pointer switch inside `corosensei`'s `resume`/`suspend` itself, during
//! which `GC_set_stackbottom`'s recorded target and the true SP are briefly for two
//! different stacks (see above). `switch_disable`/`switch_enable` bracket every
//! switch, in both directions, with `GC_disable`/`GC_enable`, so no collection — not this
//! thread's, not any other thread's, since `GC_dont_gc` is a single global counter
//! bdwgc's own docs say "overrides explicit `GC_gcollect()` calls as well" — can run while
//! a switch, or the `enter_fiber`/`leave_fiber` bookkeeping bracketing it, is in flight
//! anywhere in the process.
//!
//! Pairing is exactly symmetric per switch and split across two participants, because
//! only the fiber's own code can say a jump onto its stack has landed (see above): the
//! resumer's `switch_disable()` in `crate::scheduler::resume_fiber`, called before
//! `coroutine.resume()`, is matched by the FIBER's own `switch_enable()` once it has
//! landed and finished `enter_fiber`'s bookkeeping (`crate::scheduler::new_fiber`'s entry,
//! and the second half of `suspend_on`); the fiber's own `switch_disable()` before it
//! suspends or returns, once `leave_fiber`'s bookkeeping is done, is matched by the
//! RESUMER's `switch_enable()` once `coroutine.resume()` returns. Both bdwgc functions
//! take the collector's global allocation lock on entry and exit (`misc.c`'s `GC_enable`/
//! `GC_disable`: `LOCK(); GC_dont_gc±=1; UNLOCK();`) — measured at about half a
//! microsecond per switch, nothing on allocation, every `runtime_speed` corpus within
//! noise (none of them switch fibers at all).
//!
//! `SWITCH_DEPTH` is a per-thread counter, incremented by `switch_disable` and
//! decremented by `switch_enable`, asserted in debug builds to be exactly `0` immediately
//! before every `switch_disable()` (proving the previous switch's `switch_enable()`
//! already ran) and exactly `1` immediately before every `switch_enable()` (proving
//! exactly one switch is open) — catching a broken pairing here, in our own bookkeeping,
//! before it corrupts bdwgc's real, global `GC_dont_gc` counter (which has no such
//! assertion in a non-`GC_ASSERTIONS` build: an unmatched `GC_enable()` decrements a
//! signed counter with only a compiled-out `GC_ASSERT`, silently going negative).

use crate::mem::__gc_init;
use crate::stack_overflow;
use std::cell::Cell;
use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

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
    fn GC_disable();
    fn GC_enable();
}

thread_local! {
    /// How many [`switch_disable`] calls on this thread have not yet been matched by a
    /// [`switch_enable`]. See the module doc's switch-guard section for why this should
    /// never legitimately exceed 1, even with `run_case_guarded`/`run_fault_guarded`'s
    /// nested resumes: switches on one OS thread are always sequential, never concurrent.
    static SWITCH_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Disable collection for the span of one fiber switch (native-to-fiber resume, or
/// fiber-to-native park/exit) — see the module doc. Must be matched by exactly one later
/// [`switch_enable`] call on this same thread.
pub(crate) fn switch_disable() {
    SWITCH_DEPTH.with(|depth| {
        debug_assert_eq!(
            depth.get(),
            0,
            "gc::switch_disable called while a previous switch's GC_disable was still \
             open on this thread — the disable/enable pairing this module relies on is \
             broken"
        );
        depth.set(1);
    });
    // SAFETY: `GC_disable`/`GC_enable` nest by bdwgc's own design (a plain counter); this
    // is one well-formed call, paired with a `switch_enable` before this thread's next
    // `switch_disable`.
    unsafe { GC_disable() };
}

/// Re-enable collection, matching the [`switch_disable`] call that opened this switch.
pub(crate) fn switch_enable() {
    // SAFETY: matches the `GC_disable` this same thread's most recent `switch_disable`
    // call made — never called without one, asserted below in debug builds.
    unsafe { GC_enable() };
    SWITCH_DEPTH.with(|depth| {
        debug_assert_eq!(
            depth.get(),
            1,
            "gc::switch_enable called without a matching switch_disable on this thread"
        );
        depth.set(0);
    });
}

/// An immutable, whole-state snapshot for one thread: its live fiber ranges, the fibers
/// currently executing (outermost first, see the module doc's point (b)), and its own base
/// stack address. Published as a unit — never mutated in place — so a reader on another
/// thread ([`push_fiber_roots`], only ever running with this thread already stopped) sees
/// an internally-consistent view with no lock.
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
/// fixed-size array needs one. Generous for every worker a real machine's CPU count plus
/// the blocking pool's ceiling and the test suite's own concurrent schedulers could ever
/// register.
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

/// How many slots in [`REGISTRY`] have EVER been claimed (an upper bound on valid indices —
/// a slot at or past this count has never been claimed at all; one already claimed but not
/// yet published, or released back to [`FREE_SLOTS`], is `null`, handled by
/// [`push_fiber_roots`] the same way either way: skipped).
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Slot indices [`end_thread`] has released, available for [`my_slot`] to reuse before it
/// grows [`NEXT_SLOT`] — a helper worker thread is genuinely one-shot (spawned and joined
/// within a single `scheduler::run` call), so without reuse a long process that calls `run`
/// many times (this crate's own test suite; a host embedding the JIT repeatedly) would
/// exhaust [`MAX_THREADS`] permanently. A plain `Mutex`, not lock-free: only touched at a
/// thread's start and end, never by [`push_fiber_roots`] itself (which reads [`REGISTRY`]
/// directly), so a thread the collector stops while holding this lock can never block a
/// collection.
static FREE_SLOTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

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

/// This thread's slot index in [`REGISTRY`], claiming and initializing one on first call — a
/// released slot from [`FREE_SLOTS`] if one is waiting, otherwise a fresh one from
/// [`NEXT_SLOT`].
fn my_slot() -> usize {
    MY_SLOT.with(|cell| {
        if let Some(slot) = cell.get() {
            return slot;
        }
        let reused = FREE_SLOTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop();
        let slot = reused.unwrap_or_else(|| {
            let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
            assert!(
                slot < MAX_THREADS,
                "more threads registered with the GC fiber scanner than MAX_THREADS"
            );
            slot
        });
        let initial = Box::new(ThreadState::fresh(0));
        REGISTRY[slot].store(Box::into_raw(initial), Ordering::Release);
        cell.set(Some(slot));
        slot
    })
}

/// Release this thread's own registry slot for a later thread's [`my_slot`] to reuse. Call
/// once this thread is done running fibers for good — a worker thread whose `run`/its own
/// helper loop has ended, not a fiber-driving thread that stays alive to run `run` again
/// (the leader OS thread of a repeated `scheduler::run` call, e.g. this crate's own
/// persistent test-harness worker, keeps its slot across calls instead — nothing wrong with
/// that, just no need to churn it). A no-op if this thread never claimed one.
pub(crate) fn end_thread() {
    MY_SLOT.with(|cell| {
        let Some(slot) = cell.take() else { return };
        let old_ptr = REGISTRY[slot].swap(ptr::null_mut(), Ordering::AcqRel);
        if !old_ptr.is_null() {
            // SAFETY: this thread's own most recent publish; nulling the slot first means a
            // concurrent `push_fiber_roots` either already read the live pointer (this
            // thread not yet stopped) or sees `null` and skips the slot (freed already) —
            // never a stale read of memory this drops next.
            unsafe { drop(Box::from_raw(old_ptr)) };
        }
        FREE_SLOTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(slot);
    });
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
/// this (every worker a program's `^` runs with; every scheduler the test suite runs
/// concurrently) needs its own, while the rest of this setup need only happen the first
/// time.
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
/// thread handle. Called at the start of every worker's run (both are thread-specific).
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
