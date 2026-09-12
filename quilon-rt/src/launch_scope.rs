// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The block-scope join — the runtime half of a `< >` block joining every launch it made
//! (Quilon issue #120's "scope join", `allSettled`, never cancel).
//!
//! A block that directly launches at least one value-returning `@` primitive
//! (`@readStdin`, `@tcpRequest`) opens a launch registry on entry ([`__block_scope_enter`])
//! and joins it before its value flows out ([`__block_scope_join`]) — the code generator
//! emits both calls only around such a block (`src/deferral.rs`'s `is_launch_scope`), so
//! every other block pays nothing. [`crate::deferred::launch`] registers itself with
//! whatever scope is open at the moment it launches, so codegen never has to name a launch
//! site to register it — the registry is keyed by BLOCK, not by value, which is also what
//! lets a future primitive whose own result is ready at once but whose work is a
//! long-running background task register here the same way a value-returning launch does.
//!
//! Joining settles every registered launch — parks until each is Ready or Faulted, in
//! registration (launch) order, cancelling nothing — then, if any faulted, reports every
//! fault in that same order and exits 5. A strict force that discovers a fault before the
//! block's own close reaches a similar routine early (`fault_current_fiber`), settling
//! every scope this FIBER still has open (innermost first), so a sibling launched earlier —
//! whether in this block or one enclosing it — is settled and reported too, not abandoned.
//!
//! Scoped per FIBER, not per thread: several fibers cooperate on one OS thread (a
//! `test.it` case's own fiber today; a server's per-connection fiber and the locked
//! call-level launch tomorrow), parking and resuming interleaved, so a plain thread-local
//! stack would let one fiber's close pop a scope another fiber opened and has not closed
//! yet. Each fiber's own open scopes are kept under its own id
//! ([`crate::scheduler::current_fiber_id`]).

use crate::io::write_to_fd;
use crate::process::__exit;
use crate::report::RUNTIME_EXIT_CODE;
use std::cell::RefCell;
use std::collections::HashMap;

/// One launch's join thunk: parks until it settles, discarding the value, and hands back
/// its rendered fault report if it faulted. Boxed so a scope holds launches of different
/// result types (`Text`, `Result`, …) uniformly.
type JoinThunk = Box<dyn FnOnce() -> Option<String>>;

struct LaunchScope {
    entries: Vec<JoinThunk>,
}

thread_local! {
    /// Every fiber's own open-scope stack, keyed by fiber id — `None` is the bucket a call
    /// made outside any fiber lands in (a `quilon-rt` unit test calling these directly), so
    /// existing such tests behave exactly as before. A fiber's stack is removed once its
    /// last scope closes, so a long-running process does not accumulate one entry per
    /// fiber that has ever existed.
    static SCOPES: RefCell<HashMap<Option<usize>, Vec<LaunchScope>>> =
        RefCell::new(HashMap::new());
}

/// The key this thread's currently-running fiber (if any) tracks its own scopes under.
fn current_key() -> Option<usize> {
    crate::scheduler::current_fiber_id()
}

/// Open a launch scope for a block that directly launches — called by the code generator
/// at such a block's entry, before any of its statements run.
#[unsafe(no_mangle)]
pub extern "C" fn __block_scope_enter() {
    let key = current_key();
    SCOPES.with(|scopes| {
        scopes
            .borrow_mut()
            .entry(key)
            .or_default()
            .push(LaunchScope {
                entries: Vec::new(),
            })
    });
}

/// Register a launch's join thunk with the innermost open scope of the CURRENT fiber. A
/// no-op if none is open — reachable only when `launch` is called directly (a `quilon-rt`
/// unit test), never from compiled code: the code generator only ever calls a launching `@`
/// primitive from inside a block it has already opened a scope for.
pub(crate) fn register(join: impl FnOnce() -> Option<String> + 'static) {
    let key = current_key();
    SCOPES.with(|scopes| {
        if let Some(scope) = scopes
            .borrow_mut()
            .get_mut(&key)
            .and_then(|stack| stack.last_mut())
        {
            scope.entries.push(Box::new(join));
        }
    });
}

/// Close the innermost open scope of the CURRENT fiber — called by the code generator
/// right before a launching block's value flows out. `allSettled`: joins every registered
/// launch (waits for each to finish; none is cancelled), then reports every fault among
/// them, in launch order, and exits 5 — but only once every launch has settled, so a still-
/// pending sibling is never abandoned to report just the first.
#[unsafe(no_mangle)]
pub extern "C" fn __block_scope_join() {
    let key = current_key();
    let scope = SCOPES.with(|scopes| {
        let mut map = scopes.borrow_mut();
        let stack = map
            .get_mut(&key)
            .expect("__block_scope_join with no open scope");
        let scope = stack.pop().expect("__block_scope_join with no open scope");
        if stack.is_empty() {
            map.remove(&key);
        }
        scope
    });
    exit_on_faults(&settle_all(scope.entries));
}

/// Settle every scope the CURRENT fiber still has open, innermost first, reporting every
/// fault among them (in that combined order) and exiting 5 — without waiting for any of
/// those scopes' own close. Used when a STRICT force discovers `own_report`'s fault ahead
/// of its own block's close: the faulted value's launch is registered in one of this
/// fiber's open scopes (its own, or one enclosing it), so settling ALL of them here reaches
/// every fault a normal, unwinding close would have — a sibling launched earlier, in this
/// block or an enclosing one, is settled and reported too, not abandoned. Falls back to
/// reporting `own_report` alone when no scope is open (a `quilon-rt` unit test forcing a
/// `launch` value directly, outside any compiled block). Never returns.
pub(crate) fn fault_current_fiber(own_report: String) -> ! {
    let key = current_key();
    let stack = SCOPES.with(|scopes| scopes.borrow_mut().remove(&key));
    let faults = match stack {
        // Innermost is the LAST entry pushed — settle in that order, then outward.
        Some(stack) => stack
            .into_iter()
            .rev()
            .flat_map(|scope| settle_all(scope.entries))
            .collect(),
        None => vec![own_report],
    };
    exit_on_faults(&faults);
    unreachable!("a fiber reached through a fault always has at least that one to report")
}

/// Join every entry, in order, collecting the fault report from each that faulted.
fn settle_all(entries: Vec<JoinThunk>) -> Vec<String> {
    entries.into_iter().filter_map(|join| join()).collect()
}

/// Write every fault to stderr, in order, and exit 5 — a no-op (returns normally) when
/// `faults` is empty.
fn exit_on_faults(faults: &[String]) {
    if faults.is_empty() {
        return;
    }
    for report in faults {
        let _ = write_to_fd(2, report.as_bytes());
    }
    __exit(RUNTIME_EXIT_CODE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::cell::Cell;
    use std::os::raw::{c_int, c_void};
    use std::rc::Rc;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Pop and discard every open scope — a test that panics mid-way (an `assert_eq!`
    /// failure) would otherwise leave one open, and thread-locals in the test harness's
    /// thread-per-test model start fresh, but this keeps each test independent of that.
    fn reset() {
        SCOPES.with(|scopes| scopes.borrow_mut().clear());
    }

    #[test]
    fn join_with_no_registered_launches_settles_nothing() {
        reset();
        __block_scope_enter();
        let scope = SCOPES
            .with(|scopes| scopes.borrow_mut().get_mut(&None).and_then(|s| s.pop()))
            .unwrap();
        assert!(settle_all(scope.entries).is_empty());
    }

    #[test]
    fn join_runs_every_registered_launch_even_when_none_fault() {
        reset();
        __block_scope_enter();
        let ran = Rc::new(Cell::new(0));
        for _ in 0..3 {
            let ran = ran.clone();
            register(move || {
                ran.set(ran.get() + 1);
                None
            });
        }
        let scope = SCOPES
            .with(|scopes| scopes.borrow_mut().get_mut(&None).and_then(|s| s.pop()))
            .unwrap();
        let faults = settle_all(scope.entries);
        assert!(faults.is_empty(), "no launch faulted");
        assert_eq!(ran.get(), 3, "every registered launch must be joined");
    }

    #[test]
    fn register_with_no_open_scope_drops_the_launch_silently() {
        reset();
        let ran = Rc::new(Cell::new(false));
        let ran_clone = ran.clone();
        register(move || {
            ran_clone.set(true);
            None
        });
        // No scope was open to hold it, so it is simply never invoked — a `quilon-rt` unit
        // test calling `launch` directly outside any compiled block, not a leak the code
        // generator could ever actually trigger (see `register`'s own doc).
        assert!(!ran.get());
    }

    #[test]
    fn faults_settle_in_launch_order_regardless_of_which_settles_first() {
        reset();
        __block_scope_enter();
        // Register three launches where the MIDDLE one is the slow one to settle (it is
        // asked LAST here, simulating a sibling that finishes after its neighbors) — launch
        // order (registration order) must still win over settle order.
        register(|| Some("first".to_string()));
        register(|| None);
        register(|| Some("third".to_string()));
        let scope = SCOPES
            .with(|scopes| scopes.borrow_mut().get_mut(&None).and_then(|s| s.pop()))
            .unwrap();
        let faults = settle_all(scope.entries);
        assert_eq!(faults, vec!["first".to_string(), "third".to_string()]);
    }

    #[test]
    fn a_nested_scope_joins_independently_of_its_enclosing_one() {
        reset();
        __block_scope_enter(); // the outer block's scope
        register(|| Some("outer".to_string()));
        __block_scope_enter(); // a nested block's own scope
        register(|| Some("inner".to_string()));

        // The nested block closes first: only ITS entry is joined, the outer's is untouched.
        let inner = SCOPES
            .with(|scopes| scopes.borrow_mut().get_mut(&None).and_then(|s| s.pop()))
            .unwrap();
        assert_eq!(settle_all(inner.entries), vec!["inner".to_string()]);

        let outer = SCOPES
            .with(|scopes| scopes.borrow_mut().get_mut(&None).and_then(|s| s.pop()))
            .unwrap();
        assert_eq!(settle_all(outer.entries), vec!["outer".to_string()]);
    }

    // GC test harness — see `deferred.rs`'s own copy of this for the rationale (a stable,
    // Boehm-registered worker thread every GC-touching test body runs on).
    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }
    #[link(name = "gc", kind = "static")]
    unsafe extern "C" {
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }
    type Job = Box<dyn FnOnce() + Send>;
    fn gc_worker() -> &'static mpsc::Sender<Job> {
        static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                crate::gc::install_hooks();
                let mut stack_base = GcStackBase {
                    mem_base: std::ptr::null_mut(),
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

    #[test]
    fn two_fibers_interleaving_scopes_each_join_only_their_own() {
        // Fiber A opens, then closes first, while fiber B's OWN scope (opened in between)
        // is still open underneath it — a thread-local (not per-fiber) stack would hand
        // A's close fiber B's still-open scope instead of A's own. `a-thunk`/`b-thunk` are
        // pushed from INSIDE each fiber's own registered join thunk, so they land inside
        // that fiber's own [start, end) window in the log only if its own close joined its
        // own scope.
        static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        LOG.lock().unwrap().clear();

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    __block_scope_enter();
                    register(|| {
                        LOG.lock().unwrap().push("a-thunk");
                        None
                    });
                    sleep(Duration::from_millis(5));
                    LOG.lock().unwrap().push("a-close-start");
                    __block_scope_join();
                    LOG.lock().unwrap().push("a-close-end");
                });
                spawn(|| {
                    sleep(Duration::from_millis(1)); // let fiber A open first
                    __block_scope_enter();
                    register(|| {
                        LOG.lock().unwrap().push("b-thunk");
                        None
                    });
                    sleep(Duration::from_millis(20)); // still open when fiber A closes
                    LOG.lock().unwrap().push("b-close-start");
                    __block_scope_join();
                    LOG.lock().unwrap().push("b-close-end");
                });
            });
        });

        let log = LOG.lock().unwrap();
        let index_of = |tag: &str| {
            log.iter()
                .position(|&e| e == tag)
                .unwrap_or_else(|| panic!("missing {tag} in {log:?}"))
        };
        let (a_start, a_thunk, a_end) = (
            index_of("a-close-start"),
            index_of("a-thunk"),
            index_of("a-close-end"),
        );
        let (b_start, b_thunk, b_end) = (
            index_of("b-close-start"),
            index_of("b-thunk"),
            index_of("b-close-end"),
        );
        assert!(
            a_start < a_thunk && a_thunk < a_end,
            "fiber A's own close must join its own thunk, not fiber B's: {log:?}"
        );
        assert!(
            b_start < b_thunk && b_thunk < b_end,
            "fiber B's own close must join its own thunk, not fiber A's: {log:?}"
        );
    }
}
