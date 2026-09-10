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
//! block's own close reaches the identical routine early (`fault_innermost_scope`), so a
//! sibling launched earlier is settled and reported too, not abandoned.

use crate::io::write_to_fd;
use crate::process::__exit;
use crate::report::RUNTIME_EXIT_CODE;
use std::cell::RefCell;

/// One launch's join thunk: parks until it settles, discarding the value, and hands back
/// its rendered fault report if it faulted. Boxed so a scope holds launches of different
/// result types (`Text`, `Result`, …) uniformly.
type JoinThunk = Box<dyn FnOnce() -> Option<String>>;

struct LaunchScope {
    entries: Vec<JoinThunk>,
}

thread_local! {
    /// Open launch scopes, innermost last — a block's own scope is on top for exactly the
    /// span of its own execution (pushed on entry, popped on close), so a launch registers
    /// with the block that lexically contains it even through arbitrarily nested control
    /// flow, as long as nothing else opens a scope in between (see `register`'s own doc).
    static SCOPES: RefCell<Vec<LaunchScope>> = const { RefCell::new(Vec::new()) };
}

/// Open a launch scope for a block that directly launches — called by the code generator
/// at such a block's entry, before any of its statements run.
#[unsafe(no_mangle)]
pub extern "C" fn __block_scope_enter() {
    SCOPES.with(|scopes| {
        scopes.borrow_mut().push(LaunchScope {
            entries: Vec::new(),
        })
    });
}

/// Register a launch's join thunk with the innermost open scope. A no-op if none is open —
/// reachable only when `launch` is called directly (a `quilon-rt` unit test), never from
/// compiled code: the code generator only ever calls a launching `@` primitive from inside
/// a block it has already opened a scope for.
pub(crate) fn register(join: impl FnOnce() -> Option<String> + 'static) {
    SCOPES.with(|scopes| {
        if let Some(scope) = scopes.borrow_mut().last_mut() {
            scope.entries.push(Box::new(join));
        }
    });
}

/// Close the innermost open scope — called by the code generator right before a launching
/// block's value flows out. `allSettled`: joins every registered launch (waits for each to
/// finish; none is cancelled), then reports every fault among them, in launch order, and
/// exits 5 — but only once every launch has settled, so a still-pending sibling is never
/// abandoned to report just the first.
#[unsafe(no_mangle)]
pub extern "C" fn __block_scope_join() {
    let scope = SCOPES
        .with(|scopes| scopes.borrow_mut().pop())
        .expect("__block_scope_join with no open scope");
    exit_on_faults(&settle_all(scope.entries));
}

/// Settle the innermost open scope's launches right now, reporting every fault among them
/// (in launch order) and exiting 5 — without waiting for that scope's own close. Used when a
/// STRICT force discovers `own_report`'s fault ahead of the block's own close: the faulted
/// value's launch is already registered there, so this reaches the very same report set the
/// scope's eventual close would, and a sibling launched earlier is settled and reported too,
/// not abandoned. Falls back to reporting `own_report` alone when no scope is open (a
/// `quilon-rt` unit test forcing a `launch` value directly, outside any compiled block).
/// Never returns.
pub(crate) fn fault_innermost_scope(own_report: String) -> ! {
    let entries = SCOPES.with(|scopes| {
        scopes
            .borrow_mut()
            .last_mut()
            .map(|scope| std::mem::take(&mut scope.entries))
    });
    let faults = match entries {
        Some(entries) => settle_all(entries),
        None => vec![own_report],
    };
    exit_on_faults(&faults);
    unreachable!("a scope reached through a fault always has at least that one to report")
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
    use std::cell::Cell;
    use std::rc::Rc;

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
        let scope = SCOPES.with(|scopes| scopes.borrow_mut().pop()).unwrap();
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
        let scope = SCOPES.with(|scopes| scopes.borrow_mut().pop()).unwrap();
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
        let scope = SCOPES.with(|scopes| scopes.borrow_mut().pop()).unwrap();
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
        let inner = SCOPES.with(|scopes| scopes.borrow_mut().pop()).unwrap();
        assert_eq!(settle_all(inner.entries), vec!["inner".to_string()]);

        let outer = SCOPES.with(|scopes| scopes.borrow_mut().pop()).unwrap();
        assert_eq!(settle_all(outer.entries), vec!["outer".to_string()]);
    }
}
