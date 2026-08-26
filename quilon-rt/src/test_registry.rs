// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The test harness's event sink, driven by `core.test`'s `describe` / `it` / matchers.
//!
//! It exists because a test run needs state that outlives the call it is recorded in — how
//! deep the `describe` nesting is, whether the case in progress has failed, how many cases
//! passed and failed overall — and Quilon has no such storage: a top-level `:=` binding
//! does not persist across function calls. So the counters live here, and the language side
//! is the thin part.
//!
//! It counts and nests; it renders NOTHING. That is the reporter seam: what a run looks
//! like is decided entirely by the `.qn` reporter that reads these numbers back (the
//! default one is in `corelib/test.qn`), not here.
//!
//! State is per THREAD, so running one suite per thread isolates them completely — which is
//! how `quilon test` keeps one file's totals out of the next file's summary, and how
//! parallel Rust tests running programs in one process stay independent.
//!
//! Every primitive takes no arguments and returns an `f64`, Quilon's one number type, so
//! the code generator lowers the whole family through a single path.

use std::cell::Cell;

thread_local! {
    /// How many `describe` groups are open, for a reporter to indent by.
    static DEPTH: Cell<i64> = const { Cell::new(0) };
    /// Failures noted in the case currently running (0 while it is passing).
    static CASE_FAILURES: Cell<i64> = const { Cell::new(0) };
    /// Cases that ended with no failure noted.
    static PASSED: Cell<i64> = const { Cell::new(0) };
    /// Cases that ended with at least one.
    static FAILED: Cell<i64> = const { Cell::new(0) };
}

/// Read, replace, and hand back the new value of one counter.
fn update(counter: &'static std::thread::LocalKey<Cell<i64>>, delta: i64) -> i64 {
    counter.with(|cell| {
        let updated = cell.get() + delta;
        cell.set(updated);
        updated
    })
}

/// Open a `describe` group; yields the resulting nesting depth (1 for the outermost).
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_enter() -> f64 {
    update(&DEPTH, 1) as f64
}

/// Close a `describe` group; yields the remaining nesting depth. Clamped at 0, so an
/// unbalanced close cannot drive a reporter's indentation negative.
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_leave() -> f64 {
    DEPTH.with(|cell| {
        let remaining = (cell.get() - 1).max(0);
        cell.set(remaining);
        remaining as f64
    })
}

/// Begin a case: forget the previous one's failures. Yields the depth it sits at, which is
/// what a reporter indents the case by.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_enter() -> f64 {
    CASE_FAILURES.with(|cell| cell.set(0));
    DEPTH.with(Cell::get) as f64
}

/// End a case, folding it into the run's totals. Yields 1 when it failed, 0 when it passed.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_leave() -> f64 {
    let failures = CASE_FAILURES.with(Cell::get);
    match failures {
        0 => {
            update(&PASSED, 1);
            0.0
        }
        _ => {
            update(&FAILED, 1);
            1.0
        }
    }
}

/// Note a failure against the case in progress; yields how many it has now.
#[unsafe(no_mangle)]
pub extern "C" fn __test_note_fail() -> f64 {
    update(&CASE_FAILURES, 1) as f64
}

/// How many cases have passed so far.
#[unsafe(no_mangle)]
pub extern "C" fn __test_passed() -> f64 {
    PASSED.with(Cell::get) as f64
}

/// How many cases have failed so far.
#[unsafe(no_mangle)]
pub extern "C" fn __test_failed() -> f64 {
    FAILED.with(Cell::get) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_with_no_failure_counts_as_passed() {
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_case_enter(), 1.0);
        assert_eq!(__test_case_leave(), 0.0);
        assert_eq!(__test_passed(), 1.0);
        assert_eq!(__test_failed(), 0.0);
        assert_eq!(__test_suite_leave(), 0.0);
    }

    #[test]
    fn failures_are_scoped_to_the_case_that_noted_them() {
        __test_case_enter();
        assert_eq!(__test_note_fail(), 1.0);
        assert_eq!(__test_note_fail(), 2.0);
        assert_eq!(__test_case_leave(), 1.0);

        // The next case starts clean, so the earlier failure does not follow it.
        __test_case_enter();
        assert_eq!(__test_case_leave(), 0.0);
        assert_eq!(__test_failed(), 1.0);
        assert_eq!(__test_passed(), 1.0);
    }

    #[test]
    fn nesting_depth_never_goes_negative() {
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_suite_enter(), 2.0);
        assert_eq!(__test_suite_leave(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
    }
}
