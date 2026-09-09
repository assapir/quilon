// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The test harness's event sink, driven by `core.test`'s `describe` and `it` and by the
//! compiler-provided `expect`. This half only RENDERS each event the way the active
//! [`Reporter`] asks — the human report `docs/corelib/test/README.md` shows, or one JSON
//! object per line for a tool — taking the name, path, depth, and pass/fail flag it needs
//! as arguments. The run's own state — the counters and the open `describe` path — lives in
//! `:=` globals in `corelib/test.qn`, one program-wide cell each: `quilon test` runs one
//! suite per process and the JIT harness runs each program in its own module, so a global
//! is already per run and needs no lock.
//!
//! Three thread-locals stay here, still per-thread (which keeps parallel runs in one
//! process independent), because nothing in `.qn` writes them:
//! - [`CASE_FAILURE`] is written from the compiler-emitted `__expect_failed` path
//!   (`crate::report`), not from `describe`/`it`.
//! - [`REPORTER`] and [`SELECTION`] are written by the `quilon test` CLI before the run
//!   starts, through [`set_reporter`]/[`set_selection`]; the harness itself never sees them.
//!
//! A case carries a failed flag: a failing `expect` sets it, and the case's close tallies it
//! as passed or failed. What ENDS a case at its first failing `expect` is a different
//! mechanism — [`__test_case_run_guarded`] runs the case's body on its own nested fiber
//! (`crate::scheduler::run_case_guarded`), which a failing `expect` suspends with the abort
//! marker (`crate::scheduler::abort_current_case`) to end right there.

use std::cell::{Cell, RefCell};
use std::os::raw::c_void;

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
#[derive(Debug, Serialize)]
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
    /// What the running case's first failing `expect` recorded, if any. Native — see the
    /// module docs — because it is written from the compiler-emitted `__expect_failed` path,
    /// not from `.qn`.
    static CASE_FAILURE: RefCell<Option<Failure>> = const { RefCell::new(None) };
    /// Native — see the module docs — because the `quilon test` CLI writes it before the
    /// run starts.
    static REPORTER: Cell<Reporter> = const { Cell::new(Reporter::Human) };
    /// The `/`-joined paths the runner selected; empty selects everything. Native — see the
    /// module docs — because the `quilon test` CLI writes it before the run starts.
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
    let text = crate::mem::alloc_text(&line);
    __print_text_fd(1, text.data as *const u8, text.len, std::ptr::null());
}

fn print_line(line: &str) {
    let text = crate::mem::alloc_text(line.as_bytes());
    __print_text_fd(1, text.data as *const u8, text.len, std::ptr::null());
}

fn colored(text: &str, color: &str) -> String {
    match __color_enabled(1) {
        0 => text.to_string(),
        _ => format!("\x1b[{color}m{text}\x1b[0m"),
    }
}

/// Report a `describe` group opening: `name` (indented by `depth`) for a person, or
/// `path`/`depth` as a `suite` event for a tool. `depth`/`path` are `.qn`'s own — the count
/// of groups already open, and `name` joined onto them — computed before it pushes `name`
/// onto its `openSuiteNames` global.
///
/// # Safety contract (upheld by the compiler)
/// `name`/`path` are each null or point to their given length of readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_enter(
    name: *const u8,
    name_length: i64,
    path: *const u8,
    path_length: i64,
    depth: f64,
) -> f64 {
    let name = text_str(name, name_length);
    let path = text_str(path, path_length);
    let depth = depth as usize;
    match REPORTER.with(Cell::get) {
        Reporter::Human => print_line(&format!("{}{name}", "  ".repeat(depth))),
        Reporter::Json => emit(&Event::Suite { path: &path, depth }),
    }
    0.0
}

/// Whether the group at `path` is selected: it is (or lies under) a selected path, or a
/// selected path lies under it. 1 or 0; 1 always when nothing was selected. `path` is
/// `.qn`'s own — the open `describe` names joined with the group's, in report-path syntax.
///
/// # Safety contract (upheld by the compiler)
/// `path` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_suite_selected(path: *const u8, length: i64) -> f64 {
    let path = text_str(path, length);
    SELECTION.with(|selection| {
        let selection = selection.borrow();
        let holds_a_selection = selection.iter().any(|selected| selects(&path, selected));
        f64::from(selection.is_empty() || covered(&path, &selection) || holds_a_selection)
    })
}

/// Whether the case at `path` is selected: it is a selected path, or lies under one. 1 or 0;
/// 1 always when nothing was selected. `path` is `.qn`'s own, in report-path syntax.
///
/// # Safety contract (upheld by the compiler)
/// `path` is null or points to `length` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_selected(path: *const u8, length: i64) -> f64 {
    let path = text_str(path, length);
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

/// Whether the case being run has already failed — 1 or 0. Defensive: once a failing
/// `expect` ends its case (see [`__test_case_run_guarded`]) by suspending it past the rest
/// of the case's own statements, a later `expect` in the same case is never reached to ask
/// this at all — kept as the fallback for wherever that does not apply.
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_failing() -> f64 {
    CASE_FAILURE.with(|failure| f64::from(failure.borrow().is_some()))
}

/// Run a case's body guarded: `function`/`environment` are a `() -> $` closure's function
/// and environment pointers, split apart by the code generator's lowering of
/// `__test_run_case(body)` (see `crate::ast::RUN_TEST_CASE` in the compiler). Runs on its
/// own nested fiber (`crate::scheduler::run_case_guarded`), so a failing `expect` anywhere
/// in the call tree — however deeply nested — can end the case right there.
///
/// # Safety contract (upheld by the compiler)
/// `function`/`environment` are the function and environment pointers of a live `() -> $`
/// closure value, exactly as the code generator represents one: `function` takes the
/// environment pointer and returns the closure's `$` result.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_run_guarded(function: *const c_void, environment: *mut c_void) {
    // SAFETY: `function` is the function pointer of a live `() -> $` closure (the caller's
    // contract, above), which is exactly this signature.
    let function: extern "C" fn(*mut c_void) -> u8 = unsafe { std::mem::transmute(function) };
    crate::scheduler::run_case_guarded(function, environment);
}

/// Report the case named by `name`/`length` that just ran, at report-path `path` and
/// nesting `depth`, `failed` (0/1) telling human report and JSON `status` alike which way
/// it went. Clears [`CASE_FAILURE`] for the next case, taking the message/file/line a
/// failure carries into the JSON event; `.qn` already read `failed` off the same mark
/// (through [`__test_case_failing`]) before tallying its own counters and calling here.
///
/// # Safety contract (upheld by the compiler)
/// `name`/`path` are each null or point to their given length of readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __test_case_finish(
    name: *const u8,
    name_length: i64,
    path: *const u8,
    path_length: i64,
    depth: f64,
    failed: f64,
) -> f64 {
    let name = text_str(name, name_length);
    let path = text_str(path, path_length);
    let failure = CASE_FAILURE.with(|failure| failure.borrow_mut().take());
    let failed = failed != 0.0;
    match REPORTER.with(Cell::get) {
        Reporter::Human => {
            let mark = match failed {
                true => colored("✗", "31"),
                false => colored("✓", "32"),
            };
            print_line(&format!("{}{mark} {name}", "  ".repeat(depth as usize)));
        }
        Reporter::Json => emit(&Event::Case {
            path: &path,
            status: if failed { "fail" } else { "pass" },
            failure: failure.as_ref(),
        }),
    }
    0.0
}

/// Report the run's totals `passed`/`failed` — `.qn`'s own counters — and yield its status:
/// 0 when every case passed, 1 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __test_summary(passed: f64, failed: f64) -> f64 {
    let (passed, failed) = (passed as i64, failed as i64);
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
        crate::test_support::text_of(name)
    }

    fn finish(name: &str, path: &str, depth: f64, failed: f64) -> f64 {
        let (name_ptr, name_len) = text(name);
        let (path_ptr, path_len) = text(path);
        __test_case_finish(name_ptr, name_len, path_ptr, path_len, depth, failed)
    }

    fn a_failure() -> Failure {
        Failure {
            message: "expected 2, got 1".to_string(),
            file: "suite.qn".to_string(),
            line: 4,
        }
    }

    #[test]
    fn the_failing_mark_does_not_leak_into_the_next_case() {
        // `CASE_FAILURE` is native — the compiler-emitted `__expect_failed` path writes it —
        // so this is the one piece of a case's tally still Rust's to verify.
        assert_eq!(__test_case_failing(), 0.0, "a fresh case has not failed");
        mark_case_failed(a_failure());
        assert_eq!(__test_case_failing(), 1.0);
        // Marking twice still leaves one recorded failure.
        mark_case_failed(a_failure());
        finish("failing", "failing", 0.0, 1.0);
        assert_eq!(
            __test_case_failing(),
            0.0,
            "closing a case clears the mark for the next one"
        );
        finish("passing", "passing", 0.0, 0.0);
        assert_eq!(__test_case_failing(), 0.0);
    }

    #[test]
    fn the_summarys_status_is_nonzero_exactly_when_something_failed() {
        assert_eq!(__test_summary(3.0, 0.0), 0.0, "every case passed");
        assert_eq!(__test_summary(2.0, 1.0), 1.0, "a failed case fails the run");
    }

    #[test]
    fn a_selection_covers_a_path_or_a_suite_above_it() {
        // Path-joining is `.qn`'s job now (`corelib/test.qn`'s `pathTo`) — these primitives
        // only compare the path they are handed against the selection.
        set_selection(vec!["outer/inner".to_string(), "outer/direct".to_string()]);
        let selected = |path: &str| {
            let (pointer, length) = text(path);
            __test_case_selected(pointer, length)
        };
        let suite_selected = |path: &str| {
            let (pointer, length) = text(path);
            __test_suite_selected(pointer, length)
        };
        assert_eq!(suite_selected("outer"), 1.0, "a selection lies under it");
        assert_eq!(suite_selected("other"), 0.0);
        assert_eq!(selected("outer/direct"), 1.0, "named exactly");
        assert_eq!(
            selected("outer/directory"),
            0.0,
            "a longer name is not a prefix match"
        );
        assert_eq!(suite_selected("outer/inner"), 1.0, "named exactly");
        assert_eq!(
            selected("outer/inner/anything"),
            1.0,
            "under a selected suite"
        );
        set_selection(Vec::new());
        assert_eq!(
            selected("outer/inner/anything"),
            1.0,
            "no selection selects everything"
        );
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
