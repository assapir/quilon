// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The test harness's event sink and reporter, driven by `core.test`'s `describe` and `it`
//! and by the compiler-provided `expect`. It counts and nests, and renders each event the
//! way the active [`Reporter`] asks — the human report `docs/corelib/test/README.md` shows,
//! or one JSON object per line for a tool.
//!
//! The state lives here because a test run needs state that outlives the call recording it,
//! and Quilon has none to offer: a top-level `:=` binding does not persist across function
//! calls. It is per thread, which keeps parallel runs in one process independent.
//!
//! A case carries a failed flag: a failing `expect` sets it, every later `expect` in the
//! same case reads it and does nothing, and the case's close tallies it as passed or failed.
//! That is what lets a run report `N passed, M failed` rather than stopping at the first
//! failure.
//!
//! The runner (`quilon test`) configures a run before it starts, through [`set_reporter`]
//! and [`set_selection`]; the harness never sees the CLI.

use std::cell::{Cell, RefCell};

use serde::Serialize;

use crate::io::{__color_enabled, __print_text_fd};
use crate::text::text_str;

/// How a run's events are rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reporter {
    /// The indented case tree and the summary line, for a person.
    #[default]
    Human,
    /// One JSON object per event, one per line, for a tool.
    Json,
}

/// What the first failing `expect` of a case recorded.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Failure {
    pub(crate) message: String,
    pub(crate) file: String,
    pub(crate) line: u64,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event<'a> {
    Suite {
        path: &'a str,
        depth: usize,
    },
    Case {
        path: &'a str,
        status: &'static str,
        #[serde(flatten)]
        failure: Option<&'a Failure>,
    },
    Summary {
        passed: i64,
        failed: i64,
    },
}

thread_local! {
    /// The names of the open `describe` groups, outermost first.
    static PATH: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Cases that ran with no failing `expect`.
    static PASSED: Cell<i64> = const { Cell::new(0) };
    /// Cases that had at least one failing `expect`.
    static FAILED: Cell<i64> = const { Cell::new(0) };
    /// What the running case's first failing `expect` recorded, if any.
    static CASE_FAILURE: RefCell<Option<Failure>> = const { RefCell::new(None) };
    static REPORTER: Cell<Reporter> = const { Cell::new(Reporter::Human) };
    /// The `/`-joined paths the runner selected; empty selects everything.
    static SELECTION: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Choose how this thread's run renders its events. The runner calls it before the run.
pub fn set_reporter(reporter: Reporter) {
    REPORTER.with(|current| current.set(reporter));
}

/// Restrict this thread's run to the cases under `paths` — each a suite or case path, the
/// names joined by `/`. An empty list runs everything.
pub fn set_selection(paths: Vec<String>) {
    SELECTION.with(|selection| *selection.borrow_mut() = paths);
}

/// The path of `name` under the open groups.
fn path_to(name: &str) -> String {
    PATH.with(|path| {
        let mut full = path.borrow().join("/");
        if !full.is_empty() {
            full.push('/');
        }
        full.push_str(name);
        full
    })
}

fn depth() -> usize {
    PATH.with(|path| path.borrow().len())
}

/// Whether the selected path `selected` names `path` itself or a suite above it.
pub fn selects(selected: &str, path: &str) -> bool {
    path == selected
        || path
            .strip_prefix(selected)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path` is a selected case, or lies under a selected suite.
fn covered(path: &str, selection: &[String]) -> bool {
    selection.iter().any(|selected| selects(selected, path))
}

fn emit(event: &Event) {
    let line = serde_json::to_vec(event).expect("a test event serializes");
    __print_text_fd(1, line.as_ptr(), line.len() as i64);
}

fn print_line(line: &str) {
    __print_text_fd(1, line.as_ptr(), line.len() as i64);
}

fn colored(text: &str, color: &str) -> String {
    match __color_enabled(1) {
        0 => text.to_string(),
        _ => format!("\x1b[{color}m{text}\x1b[0m"),
    }
}

/// Open a `describe` group named by `name`/`length`, report it, and yield the resulting
/// nesting depth (1 for the outermost).
///
/// # Safety contract (upheld by the compiler)
/// `name` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_enter(name: *const u8, length: i64) -> f64 {
    let name = text_str(name, length).into_owned();
    match REPORTER.with(Cell::get) {
        Reporter::Human => print_line(&format!("{}{name}", "  ".repeat(depth()))),
        Reporter::Json => emit(&Event::Suite {
            path: &path_to(&name),
            depth: depth(),
        }),
    }
    PATH.with(|path| {
        path.borrow_mut().push(name);
        path.borrow().len() as f64
    })
}

/// How many `describe` groups are open right now — 0 outside any group, 1 inside an
/// outermost one. Reads the depth without moving it, which is what a case asking for the
/// run's state needs (`enter`/`leave` are the harness's, and they move it).
#[unsafe(no_mangle)]
pub extern "C" fn __test_depth() -> f64 {
    depth() as f64
}

/// Close the innermost `describe` group; yields the remaining nesting depth. An unbalanced
/// close is a no-op, so the depth cannot go negative.
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_leave() -> f64 {
    PATH.with(|path| {
        path.borrow_mut().pop();
        path.borrow().len() as f64
    })
}

/// Whether the group `name` would open is selected: it is (or lies under) a selected path,
/// or a selected path lies under it. 1 or 0; 1 always when nothing was selected.
///
/// # Safety contract (upheld by the compiler)
/// `name` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_selected(name: *const u8, length: i64) -> f64 {
    let path = path_to(&text_str(name, length));
    SELECTION.with(|selection| {
        let selection = selection.borrow();
        let holds_a_selection = selection.iter().any(|selected| selects(&path, selected));
        f64::from(selection.is_empty() || covered(&path, &selection) || holds_a_selection)
    })
}

/// Whether the case `name` is selected: it is a selected path, or lies under one. 1 or 0;
/// 1 always when nothing was selected.
///
/// # Safety contract (upheld by the compiler)
/// `name` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_selected(name: *const u8, length: i64) -> f64 {
    let path = path_to(&text_str(name, length));
    SELECTION.with(|selection| {
        let selection = selection.borrow();
        f64::from(selection.is_empty() || covered(&path, &selection))
    })
}

/// Mark the case being run as failed, keeping what its FIRST failure said. Called from the
/// `expect` failure path, so the rest of the case's assertions are skipped and the report
/// has a failure to show.
pub(crate) fn mark_case_failed(failure: Failure) {
    CASE_FAILURE.with(|recorded| {
        let mut recorded = recorded.borrow_mut();
        if recorded.is_none() {
            *recorded = Some(failure);
        }
    });
}

/// Whether the case being run has already failed — 1 or 0. An `expect` asks this first and
/// evaluates nothing when the answer is 1, which is how a failure skips the rest of its case.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_failing() -> f64 {
    CASE_FAILURE.with(|failure| f64::from(failure.borrow().is_some()))
}

/// Close the case named by `name`/`length` that just ran: tally it as passed or failed,
/// report it, clear the mark for the next one, and yield the depth it sits at.
///
/// # Safety contract (upheld by the compiler)
/// `name` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_finish(name: *const u8, length: i64) -> f64 {
    let name = text_str(name, length);
    let failure = CASE_FAILURE.with(|failure| failure.borrow_mut().take());
    match failure.is_some() {
        true => FAILED.with(|count| count.set(count.get() + 1)),
        false => PASSED.with(|count| count.set(count.get() + 1)),
    }
    match REPORTER.with(Cell::get) {
        Reporter::Human => {
            let mark = match failure.is_some() {
                true => colored("✗", "31"),
                false => colored("✓", "32"),
            };
            print_line(&format!("{}{mark} {name}", "  ".repeat(depth())));
        }
        Reporter::Json => emit(&Event::Case {
            path: &path_to(&name),
            status: if failure.is_some() { "fail" } else { "pass" },
            failure: failure.as_ref(),
        }),
    }
    depth() as f64
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

/// Report the run's totals and yield its status: 0 when every case passed, 1 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __test_summary() -> f64 {
    let (passed, failed) = (PASSED.with(Cell::get), FAILED.with(Cell::get));
    match REPORTER.with(Cell::get) {
        Reporter::Human => {
            let tally = format!("{passed} passed, {failed} failed");
            print_line("");
            print_line(&colored(&tally, if failed == 0 { "32" } else { "31" }));
        }
        Reporter::Json => emit(&Event::Summary { passed, failed }),
    }
    f64::from(failed != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: &str) -> (*const u8, i64) {
        (name.as_ptr(), name.len() as i64)
    }

    fn enter(name: &str) -> f64 {
        let (pointer, length) = text(name);
        __test_suite_enter(pointer, length)
    }

    fn finish(name: &str) -> f64 {
        let (pointer, length) = text(name);
        __test_case_finish(pointer, length)
    }

    fn a_failure() -> Failure {
        Failure {
            message: "expected 2, got 1".to_string(),
            file: "suite.qn".to_string(),
            line: 4,
        }
    }

    #[test]
    fn a_case_is_counted_and_reports_the_depth_it_sits_at() {
        // Three cases across two nesting levels, so the count and the depth cannot be
        // confused for each other.
        assert_eq!(enter("outer"), 1.0);
        assert_eq!(__test_depth(), 1.0, "reading the depth does not move it");
        assert_eq!(__test_depth(), 1.0);
        assert_eq!(finish("first"), 1.0, "outermost group: depth 1");
        assert_eq!(finish("second"), 1.0, "depth does not move between cases");
        assert_eq!(enter("inner"), 2.0);
        assert_eq!(finish("third"), 2.0, "nested group: depth 2");
        assert_eq!(__test_passed(), 3.0, "all three cases counted");
        assert_eq!(__test_failed(), 0.0, "none of them failed");
        assert_eq!(__test_suite_leave(), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
    }

    #[test]
    fn nesting_depth_never_goes_negative() {
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(enter("group"), 1.0);
        assert_eq!(__test_suite_leave(), 0.0);
        assert_eq!(finish("case"), 0.0);
    }

    #[test]
    fn a_failed_case_is_tallied_apart_and_the_flag_does_not_leak() {
        assert_eq!(__test_case_failing(), 0.0, "a fresh case has not failed");
        mark_case_failed(a_failure());
        assert_eq!(__test_case_failing(), 1.0);
        // Marking twice still counts one failed case.
        mark_case_failed(a_failure());
        finish("failing");
        assert_eq!(__test_failed(), 1.0);
        assert_eq!(__test_passed(), 0.0);
        assert_eq!(
            __test_case_failing(),
            0.0,
            "the next case starts out passing"
        );
        finish("passing");
        assert_eq!(__test_passed(), 1.0);
        assert_eq!(__test_failed(), 1.0);
        assert_eq!(__test_summary(), 1.0, "a failed case fails the run");
    }

    #[test]
    fn a_selection_covers_a_case_by_its_path_or_by_a_suite_above_it() {
        set_selection(vec!["outer/inner".to_string(), "outer/direct".to_string()]);
        let selected = |name: &str| {
            let (pointer, length) = text(name);
            __test_case_selected(pointer, length)
        };
        let suite_selected = |name: &str| {
            let (pointer, length) = text(name);
            __test_suite_selected(pointer, length)
        };
        assert_eq!(suite_selected("outer"), 1.0, "a selection lies under it");
        assert_eq!(suite_selected("other"), 0.0);
        enter("outer");
        assert_eq!(selected("direct"), 1.0, "named exactly");
        assert_eq!(
            selected("directory"),
            0.0,
            "a longer name is not a prefix match"
        );
        assert_eq!(suite_selected("inner"), 1.0, "named exactly");
        enter("inner");
        assert_eq!(selected("anything"), 1.0, "under a selected suite");
        set_selection(Vec::new());
        assert_eq!(selected("anything"), 1.0, "no selection selects everything");
    }

    #[test]
    fn a_json_event_carries_a_failure_flat_and_a_pass_carries_none() {
        let pass = serde_json::to_string(&Event::Case {
            path: "a/b",
            status: "pass",
            failure: None,
        })
        .unwrap();
        assert_eq!(pass, r#"{"event":"case","path":"a/b","status":"pass"}"#);
        let fail = serde_json::to_string(&Event::Case {
            path: "a/b",
            status: "fail",
            failure: Some(&a_failure()),
        })
        .unwrap();
        assert_eq!(
            fail,
            r#"{"event":"case","path":"a/b","status":"fail","message":"expected 2, got 1","file":"suite.qn","line":4}"#
        );
    }
}
