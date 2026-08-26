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
//!     restore the main stack base. The running fiber is excluded from (a) so it is
//!     scanned once, by the automatic scan.
//!
//! Any pre-existing `GC_push_other_roots` callback is chained, not clobbered. The
//! `#[no_mangle]` GC intrinsics (`__gc_init`, the allocation binding) live in
//! [`crate::mem`]; this module holds only the scheduler-side scanning integration.

use crate::mem::__gc_init;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::sync::Mutex;

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
    fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    fn GC_allow_register_threads();
}

struct GcState {
    /// Per-fiber usable stack range `[low, high)`, indexed by fiber id (`None` when
    /// the slot is free). Mirrors the scheduler slab so the callback can scan parked
    /// fibers.
    ranges: Vec<Option<(usize, usize)>>,
    /// The fiber currently executing (excluded from the parked-fiber scan; it is
    /// covered by `GC_set_stackbottom` + Boehm's automatic scan instead).
    running: Option<usize>,
    /// The main (scheduler) thread's stack base, restored on every switch back.
    main_base: usize,
    /// Previous `GC_push_other_roots` callback, called after ours (chaining).
    previous: Option<GcPushOtherRootsProc>,
    installed: bool,
}

// A plain Mutex (not a RefCell) because the static must be `Sync` and the
// push-roots callback is FFI. It is never locked across an operation that can
// trigger a collection, so the callback can always take it without deadlock.
static GC_STATE: Mutex<GcState> = Mutex::new(GcState {
    ranges: Vec::new(),
    running: None,
    main_base: 0,
    previous: None,
    installed: false,
});

fn lock() -> std::sync::MutexGuard<'static, GcState> {
    GC_STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Boehm calls this during a collection (world stopped). It pushes every live
/// parked fiber's stack, then chains to any previously-installed callback.
extern "C" fn push_fiber_roots() {
    let previous = {
        let state = lock();
        for (id, range) in state.ranges.iter().enumerate() {
            if let Some((low, high)) = *range
                && Some(id) != state.running
            {
                // SAFETY: [low, high) is this fiber's committed, readable stack
                // (above the guard page); pushing it is what keeps its roots alive.
                // GC_push_all_eager neither allocates nor re-enters us.
                unsafe { GC_push_all_eager(low as *mut c_void, high as *mut c_void) };
            }
        }
        state.previous
    };
    // SAFETY: `previous` is the callback Boehm handed back from GC_get_push_other_roots.
    if let Some(p) = previous {
        unsafe { p() };
    }
}

/// Idempotent one-time install: initialize the GC, permit foreign-thread
/// registration, and chain in our push-roots callback.
pub(crate) fn install_hooks() {
    let mut state = lock();
    if state.installed {
        return;
    }
    __gc_init();
    // SAFETY: all one-time GC configuration on an initialized collector.
    unsafe {
        GC_allow_register_threads();
        state.previous = GC_get_push_other_roots();
        GC_set_push_other_roots(push_fiber_roots);
    }
    state.installed = true;
}

/// Reset per-run state and record the current thread's main stack base. Called at
/// the start of every scheduler run (the base is thread-specific).
pub(crate) fn begin_run() {
    let mut stack_base = GcStackBase {
        mem_base: ptr::null_mut(),
    };
    // SAFETY: `GC_get_stack_base` fills `stack_base` with the current thread's base.
    unsafe { GC_get_stack_base(&mut stack_base) };
    let mut state = lock();
    state.ranges.clear();
    state.running = None;
    state.main_base = stack_base.mem_base as usize;
}

pub(crate) fn register(id: usize, low: usize, high: usize) {
    let mut state = lock();
    if id >= state.ranges.len() {
        state.ranges.resize(id + 1, None);
    }
    state.ranges[id] = Some((low, high));
}

pub(crate) fn unregister(id: usize) {
    let mut state = lock();
    if id < state.ranges.len() {
        state.ranges[id] = None;
    }
}

/// Point Boehm's automatic stack scan at the stack whose cold end (base) is `base` —
/// the running fiber's, or the main thread's on switch-back.
fn set_stackbottom(base: usize) {
    let stack_base = GcStackBase {
        mem_base: base as *mut c_void,
    };
    // SAFETY: single-threaded; `stack_base` is read (copied) synchronously by the call.
    unsafe { GC_set_stackbottom(ptr::null_mut(), &stack_base) };
}

/// Switch Boehm's notion of "the stack" onto fiber `id` (base `high`) before
/// resuming it, and mark it running so the parked-fiber scan skips it.
pub(crate) fn enter_fiber(id: usize, high: usize) {
    lock().running = Some(id);
    set_stackbottom(high);
}

/// Restore the main stack base after a fiber yields or finishes.
pub(crate) fn leave_fiber() {
    let main_base = {
        let mut state = lock();
        state.running = None;
        state.main_base
    };
    set_stackbottom(main_base);
}
