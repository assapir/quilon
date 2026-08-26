// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The test harness's event sink, driven by `core.test`'s `describe` and `it` and by the
//! compiler-provided `expect`. It counts and nests, and renders nothing — see the reporter
//! seam in `docs/corelib/test.md`.
//!
//! The counters live here because a test run needs state that outlives the call recording
//! it, and Quilon has none to offer: a top-level `:=` binding does not persist across
//! function calls.
//!
//! A case carries a failed flag: a failing `expect` sets it, every later `expect` in the
//! same case reads it and does nothing, and the case's close tallies it as passed or failed.
//! That is what lets a run report `N passed, M failed` rather than stopping at the first
//! failure.
//!
//! State is per thread, which keeps parallel runs in one process independent.

use std::cell::Cell;

thread_local! {
    /// How many `describe` groups are open, for a reporter to indent by.
    static DEPTH: Cell<i64> = const { Cell::new(0) };
    /// Cases that ran with no failing `expect`.
    static PASSED: Cell<i64> = const { Cell::new(0) };
    /// Cases that had at least one failing `expect`.
    static FAILED: Cell<i64> = const { Cell::new(0) };
    /// Whether the case being run has already failed.
    static CASE_FAILED: Cell<bool> = const { Cell::new(false) };
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

/// Mark the case being run as failed. Called from the `expect` failure path, so the rest of
/// the case's assertions are skipped and the tally has a failure to report.
pub(crate) fn mark_case_failed() {
    CASE_FAILED.with(|failed| failed.set(true));
}

/// Whether the case being run has already failed — 1 or 0. An `expect` asks this first and
/// evaluates nothing when the answer is 1, which is how a failure skips the rest of its case.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_failing() -> f64 {
    CASE_FAILED.with(|failed| f64::from(failed.get()))
}

/// Close the case that just ran: tally it as passed or failed, clear the flag for the next
/// one, and yield the depth it sits at — which is what a reporter indents the case by.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_finish() -> f64 {
    let failed = CASE_FAILED.with(|flag| flag.replace(false));
    match failed {
        true => FAILED.with(|count| count.set(count.get() + 1)),
        false => PASSED.with(|count| count.set(count.get() + 1)),
    }
    DEPTH.with(Cell::get) as f64
}

/// How many cases have passed so far.
#[unsafe(no_mangle)]
pub extern "C" fn __test_passed() -> f64 {
    PASSED.with(Cell::get) as f64
}

/// How many cases have failed so far. Non-zero is what makes a run exit non-zero.
#[unsafe(no_mangle)]
pub extern "C" fn __test_failed() -> f64 {
    FAILED.with(Cell::get) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_is_counted_and_reports_the_depth_it_sits_at() {
        // Three cases across two nesting levels, so the count and the depth cannot be
        // confused for each other.
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_case_finish(), 1.0, "outermost group: depth 1");
        assert_eq!(
            __test_case_finish(),
            1.0,
            "depth does not move between cases"
        );
        assert_eq!(__test_suite_enter(), 2.0);
        assert_eq!(__test_case_finish(), 2.0, "nested group: depth 2");
        assert_eq!(__test_passed(), 3.0, "all three cases counted");
        assert_eq!(__test_failed(), 0.0, "none of them failed");
        assert_eq!(__test_suite_leave(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
    }

    #[test]
    fn nesting_depth_never_goes_negative() {
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(__test_suite_enter(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(__test_case_finish(), 0.0);
    }

    #[test]
    fn a_failed_case_is_tallied_apart_and_the_flag_does_not_leak() {
        assert_eq!(__test_case_failing(), 0.0, "a fresh case has not failed");
        mark_case_failed();
        assert_eq!(__test_case_failing(), 1.0);
        // Marking twice still counts one failed case.
        mark_case_failed();
        __test_case_finish();
        assert_eq!(__test_failed(), 1.0);
        assert_eq!(__test_passed(), 0.0);
        assert_eq!(
            __test_case_failing(),
            0.0,
            "the next case starts out passing"
        );
        __test_case_finish();
        assert_eq!(__test_passed(), 1.0);
        assert_eq!(__test_failed(), 1.0);
    }
}
