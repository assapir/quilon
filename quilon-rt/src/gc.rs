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
//! `enter_fiber`/`leave_fiber` run from the fiber's own code (only it can say a jump has
//! landed), not the resumer's, and each orders its two steps so the fiber is always
//! covered by (a) or (b), never neither. The registry is per thread: one immutable
//! `ThreadState` snapshot behind an `AtomicPtr`, swapped by `publish` — no lock
//! `push_fiber_roots` (running with every other thread already stopped) could ever
//! deadlock on.
//!
//! That leaves one gap neither ordering closes: the raw stack-pointer switch inside
//! `corosensei`'s `resume`/`suspend` itself, opaque to both sides, during which
//! `GC_set_stackbottom`'s target and the true SP are briefly for two different stacks.
//! `switch_disable`/`switch_enable` bracket every switch with `GC_disable`/`GC_enable` to
//! hold collection off for that instant — process-wide, since `GC_dont_gc` is one global
//! counter — paired symmetrically across resumer and fiber per switch; `SWITCH_DEPTH`
//! asserts the pairing in debug builds. About half a microsecond per switch, nothing on
//! allocation.

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
    /// Open [`switch_disable`] calls not yet matched by [`switch_enable`] — never above 1;
    /// switches on one thread are sequential even when nested (a case/trap guard).
    static SWITCH_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Hold collection off for one switch. Matched by exactly one later [`switch_enable`].
pub(crate) fn switch_disable() {
    SWITCH_DEPTH.with(|depth| {
        debug_assert_eq!(depth.get(), 0, "gc::switch_disable: pairing already open");
        depth.set(1);
    });
    // SAFETY: GC_disable/GC_enable nest by bdwgc's own design.
    unsafe { GC_disable() };
}

/// Matches the [`switch_disable`] that opened this switch.
pub(crate) fn switch_enable() {
    // SAFETY: matches this thread's own switch_disable.
    unsafe { GC_enable() };
    SWITCH_DEPTH.with(|depth| {
        debug_assert_eq!(
            depth.get(),
            1,
            "gc::switch_enable: no matching switch_disable"
        );
        depth.set(0);
    });
}

/// One thread's scan state, published as a whole so a reader on another thread (always
/// stopped first) never sees a half-updated view.
struct ThreadState {
    /// Per-fiber usable stack range, by fiber id. `Arc`'d: `running` changes every switch,
    /// `ranges` only on register/unregister.
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

/// Fixed registry size — covers any realistic CPU count plus the blocking pool and the
/// test suite's own concurrent schedulers.
const MAX_THREADS: usize = 256;

/// One slot per thread, written only by its owner. `AtomicPtr`, not a mutex:
/// `push_fiber_roots` reads every slot with no lock a stopped thread could hold.
static REGISTRY: [AtomicPtr<ThreadState>; MAX_THREADS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const NULL: AtomicPtr<ThreadState> = AtomicPtr::new(ptr::null_mut());
    [NULL; MAX_THREADS]
};

/// Slots ever claimed — a slot past this is unclaimed; one claimed but not yet published,
/// or released, is `null`, and [`push_fiber_roots`] skips it either way.
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Slots [`end_thread`] released, for [`my_slot`] to reuse before growing [`NEXT_SLOT`] —
/// a helper worker thread is one-shot, so without reuse a process calling `run` many times
/// (this crate's own test suite) exhausts [`MAX_THREADS`].
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

/// Release this thread's slot for reuse. Call once a worker's loop has ended for good —
/// the leader thread of a repeated `run` call keeps its slot instead.
pub(crate) fn end_thread() {
    MY_SLOT.with(|cell| {
        let Some(slot) = cell.take() else { return };
        let old_ptr = REGISTRY[slot].swap(ptr::null_mut(), Ordering::AcqRel);
        if !old_ptr.is_null() {
            // SAFETY: nulled first, so a concurrent push_fiber_roots sees either the live
            // pointer or null, never this freed one.
            unsafe { drop(Box::from_raw(old_ptr)) };
        }
        FREE_SLOTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(slot);
    });
}

fn my_state() -> &'static ThreadState {
    let ptr = REGISTRY[my_slot()].load(Ordering::Acquire);
    // SAFETY: published by this thread, the slot's only writer.
    unsafe { &*ptr }
}

/// Publish a new snapshot built from the current one, freeing the one it replaces.
fn publish(f: impl FnOnce(&ThreadState) -> ThreadState) {
    let slot = my_slot();
    let old_ptr = REGISTRY[slot].load(Ordering::Acquire);
    // SAFETY: this thread's own previous publish.
    let old = unsafe { &*old_ptr };
    let new_ptr = Box::into_raw(Box::new(f(old)));
    REGISTRY[slot].store(new_ptr, Ordering::Release);
    // SAFETY: only this thread frees its own slot; a reader elsewhere only ever runs with
    // this thread already stopped.
    unsafe { drop(Box::from_raw(old_ptr)) };
}

/// Boehm's world-stopped callback: pushes every registered thread's non-running fiber
/// ranges, then chains to whatever callback was installed before.
extern "C" fn push_fiber_roots() {
    let used = NEXT_SLOT.load(Ordering::Acquire).min(MAX_THREADS);
    for slot in &REGISTRY[..used] {
        let ptr = slot.load(Ordering::Acquire);
        if ptr.is_null() {
            continue;
        }
        // SAFETY: this thread is frozen for the whole call.
        let state = unsafe { &*ptr };
        for (id, range) in state.ranges.iter().enumerate() {
            if let Some((low, high)) = *range
                && state.running.last() != Some(&id)
            {
                // SAFETY: a committed, readable range; GC_push_all_eager neither
                // allocates nor re-enters us.
                unsafe { GC_push_all_eager(low as *mut c_void, high as *mut c_void) };
            }
        }
    }
    // SAFETY: set before our callback is ever installed.
    if let Some(previous) = PREVIOUS_CALLBACK.get().copied().flatten() {
        unsafe { previous() };
    }
}

/// Per-thread setup (guard-page handler; `stack_overflow::install` is itself per-thread)
/// plus the process's one-time GC configuration.
pub(crate) fn install_hooks() {
    stack_overflow::install();

    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        __gc_init();
        // SAFETY: one-time GC configuration on an initialized collector.
        unsafe {
            GC_allow_register_threads();
            let previous = GC_get_push_other_roots();
            let _ = PREVIOUS_CALLBACK.set(previous);
            GC_set_push_other_roots(push_fiber_roots);
        }
    });
}

/// Reset per-run state and record this thread's stack base and GC handle.
pub(crate) fn begin_run() {
    let mut stack_base = GcStackBase {
        mem_base: ptr::null_mut(),
    };
    // SAFETY: the thread must already be registered with the collector.
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

/// Called from the fiber's own code, right after it lands — never from the resumer's side,
/// which cannot observe when the jump has landed. Points the automatic scan at this fiber's
/// stack, then excludes it from the parked-fiber scan, so it is always covered by one or the
/// other. See the module doc.
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

/// Called from the fiber's own code, as the last act before it suspends or returns — mirrors
/// [`enter_fiber`]'s ordering, re-included in the parked-fiber scan before stackbottom is
/// handed back. See the module doc.
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
