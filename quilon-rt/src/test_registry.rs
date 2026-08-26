// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The test harness's event sink, driven by `core.test`'s `describe` and `it`. It counts and
//! nests, and renders nothing — see the reporter seam in `docs/corelib/test.md`.
//!
//! The counters live here because a test run needs state that outlives the call recording
//! it, and Quilon has none to offer: a top-level `:=` binding does not persist across
//! function calls.
//!
//! A case is counted once it returns, assertions being fail-fast — a failing one reports and
//! exits 101, so nothing after it runs. Hence no failure counter: a run that reaches the
//! summary had none.
//!
//! State is per thread, which keeps parallel runs in one process independent.

use std::cell::Cell;

thread_local! {
    /// How many `describe` groups are open, for a reporter to indent by.
    static DEPTH: Cell<i64> = const { Cell::new(0) };
    /// Cases that have run to completion.
    static PASSED: Cell<i64> = const { Cell::new(0) };
}

/// Open a `describe` group; yields the resulting nesting depth (1 for the outermost).
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_enter() -> f64 {
    DEPTH.with(|depth| {
        depth.set(depth.get() + 1);
        depth.get() as f64
    })
}

/// Close a `describe` group; yields the remaining nesting depth. Clamped at 0, so an
/// unbalanced close cannot drive a reporter's indentation negative.
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_leave() -> f64 {
    DEPTH.with(|depth| {
        depth.set((depth.get() - 1).max(0));
        depth.get() as f64
    })
}

/// Count a case that ran to completion — and so passed, assertions being fail-fast. Yields
/// the depth it sits at, which is what a reporter indents the case by.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_passed() -> f64 {
    PASSED.with(|passed| passed.set(passed.get() + 1));
    DEPTH.with(Cell::get) as f64
}

/// How many cases have passed so far.
#[unsafe(no_mangle)]
pub extern "C" fn __test_passed() -> f64 {
    PASSED.with(Cell::get) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_is_counted_where_it_sits() {
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_case_passed(), 1.0);
        assert_eq!(__test_suite_enter(), 2.0);
        assert_eq!(__test_case_passed(), 2.0);
        assert_eq!(__test_passed(), 2.0);
        assert_eq!(__test_suite_leave(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
    }

    #[test]
    fn nesting_depth_never_goes_negative() {
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(__test_case_passed(), 0.0);
    }
}
