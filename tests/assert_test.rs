//! `core.test` assertion module: end-to-end exit-code behavior.
//!
//! A passing assertion runs to completion (exit 0); a FAILING assertion prints a
//! located report to stderr and exits **101** (the Rust-panic convention `core.test`
//! uses, distinct from the small result codes examples use as their normal exit status).
//!
//! The report names the failing call's own `file:line:column` and underlines it, and the
//! location is the USER's call site even through a wrapper (`assertEq` reports where the
//! program called `assertEq`, not where `core.test` calls `failAt`). Those are the cases
//! under "Call-site reporting" below.
//!
//! Every case is driven as a SUBPROCESS — `quilon run <file>` (in-process JIT) and,
//! where a linker is present, `quilon build` + execute (native AOT). It must never be
//! the in-process `jit::run_program`, because a failing assertion calls the `__exit`
//! runtime intrinsic (libc `exit`), which would terminate the test runner itself.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::ensure_runtime_lib;

const FAIL_CODE: i32 = 101;

fn quilon() -> &'static str {
    env!("CARGO_BIN_EXE_quilon")
}

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quilon_assert_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The `file:line:column:` position line a report prints for `path` — with the path
/// elided exactly as the report elides it.
///
/// A report shortens a path wider than `MAX_PATH_WIDTH` from its start, so an
/// expectation built from the raw path only holds where the temp directory is short.
/// Linux's `/tmp/...` always is; macOS's `/var/folders/<random>/T/...` never is, which
/// is where an expectation spelled with `path.display()` fails while the compiler is
/// behaving exactly as documented.
fn position(path: &Path, line: u32, column: u32) -> String {
    let shown = quilon::source_map::shorten_path(&path.display().to_string());
    format!("{shown}:{line}:{column}:")
}

/// Write `src` to a uniquely-named `.qn` file under the per-process temp dir.
fn write_program(tag: &str, src: &str) -> PathBuf {
    let path = tmp_dir().join(format!("{tag}.qn"));
    let mut f = std::fs::File::create(&path).expect("create .qn");
    f.write_all(src.as_bytes()).expect("write .qn");
    path
}

/// `(exit_code, stderr)` from `quilon run <file>` (in-process JIT, as a subprocess).
fn run_jit(tag: &str, src: &str) -> (i32, String) {
    let path = write_program(tag, src);
    let out = Command::new(quilon())
        .args(["run", path.to_str().unwrap()])
        .output()
        .expect("spawn quilon run");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Is a linker available on PATH? (Mirrors the examples gate's graceful skip.)
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// `(exit_code, stderr)` from a native AOT build (`quilon build --linker <linker>`)
/// then executing the resulting binary.
fn run_aot(tag: &str, src: &str, linker: &str) -> (i32, String) {
    let path = write_program(tag, src);
    let bin = tmp_dir().join(format!("{tag}.{linker}.bin"));
    let build = Command::new(quilon())
        .args(["build", path.to_str().unwrap(), "--linker", linker])
        .args(["-o", bin.to_str().unwrap()])
        .output()
        .expect("spawn quilon build");
    assert!(
        build.status.success(),
        "{tag}: `quilon build --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&bin).output().expect("run native binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// --- The primitive: assert -------------------------------------------------

#[test]
fn passing_assert_exits_zero() {
    let (code, _) = run_jit(
        "assert_pass",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 2)\n",
    );
    assert_eq!(code, 0, "a passing assert must exit 0");
}

#[test]
fn failing_assert_exits_101_with_stderr_message() {
    let (code, stderr) = run_jit(
        "assert_fail",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 3)\n",
    );
    assert_eq!(code, FAIL_CODE, "a failing assert must exit {FAIL_CODE}");
    assert!(
        stderr.contains("assertion failed"),
        "failing assert must print the default message to stderr, got: {stderr:?}"
    );
}

#[test]
fn passing_assert_with_message_exits_zero() {
    // The `AssertOpts` message overload: a holding condition still does nothing.
    let (code, _) = run_jit(
        "assert_msg_pass",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 2, AssertOpts { message = \"unused\" })\n",
    );
    assert_eq!(code, 0, "a passing assert(cond, opts) must exit 0");
}

#[test]
fn failing_assert_with_message_prints_that_message() {
    // The `AssertOpts` message overload prints opts.message (not the default) and
    // still exits 101.
    let (code, stderr) = run_jit(
        "assert_msg_fail",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 3, AssertOpts { message = \"custom boom\" })\n",
    );
    assert_eq!(
        code, FAIL_CODE,
        "a failing assert(cond, opts) must exit {FAIL_CODE}"
    );
    assert!(
        stderr.contains("custom boom"),
        "the message overload must print opts.message to stderr, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("assertion failed"),
        "the message overload must NOT print the default message, got: {stderr:?}"
    );
}

// --- assertEq / assertNotEq ------------------------------------------------

#[test]
fn assert_eq_pass_and_fail() {
    // Num.
    let (code, _) = run_jit(
        "eq_num_pass",
        "<< core.test\n^ = () -> $ => assertEq(6 * 7, 42)\n",
    );
    assert_eq!(code, 0);

    let (code, stderr) = run_jit(
        "eq_num_fail",
        "<< core.test\n^ = () -> $ => assertEq(6 * 7, 41)\n",
    );
    assert_eq!(code, FAIL_CODE);
    // Failure message renders expected (41) and actual (42) via eprint.
    assert!(
        stderr.contains("41") && stderr.contains("42"),
        "assertEq failure must show expected vs actual, got: {stderr:?}"
    );

    // Text and Bool overloads.
    let (code, _) = run_jit(
        "eq_text_pass",
        "<< core.test\n^ = () -> $ => assertEq(\"a\" + \"b\", \"ab\")\n",
    );
    assert_eq!(code, 0);
    let (code, _) = run_jit(
        "eq_bool_fail",
        "<< core.test\n^ = () -> $ => assertEq(1 < 2, false)\n",
    );
    assert_eq!(code, FAIL_CODE);
}

#[test]
fn assert_not_eq_pass_and_fail() {
    let (code, _) = run_jit(
        "ne_pass",
        "<< core.test\n^ = () -> $ => assertNotEq(1, 2)\n",
    );
    assert_eq!(code, 0);

    let (code, stderr) = run_jit(
        "ne_fail",
        "<< core.test\n^ = () -> $ => assertNotEq(5, 5)\n",
    );
    assert_eq!(code, FAIL_CODE);
    assert!(stderr.contains("assertion failed"), "got: {stderr:?}");
}

// --- assertOk / assertNotOk ------------------------------------------------

#[test]
fn assert_ok_pass_and_fail() {
    let (code, _) = run_jit(
        "ok_pass",
        "<< core.test\n^ = () -> $ => assertOk([10, 20].at(0))\n",
    );
    assert_eq!(code, 0);

    let (code, _) = run_jit(
        "ok_fail",
        "<< core.test\n^ = () -> $ => assertOk([10, 20].at(9))\n",
    );
    assert_eq!(code, FAIL_CODE);
}

#[test]
fn assert_not_ok_pass_and_fail() {
    let (code, _) = run_jit(
        "notok_pass",
        "<< core.test\n^ = () -> $ => assertNotOk([10, 20].at(9))\n",
    );
    assert_eq!(code, 0);

    let (code, _) = run_jit(
        "notok_fail",
        "<< core.test\n^ = () -> $ => assertNotOk([10, 20].at(0))\n",
    );
    assert_eq!(code, FAIL_CODE);
}

// --- Call-site reporting ---------------------------------------------------

/// The whole report for one known failure, line for line: position, message, gutter,
/// source line, caret run. Pinning the exact shape is the point — this is the output a
/// person reads when a test fails, and it must stay a compiler-style diagnostic.
#[test]
fn failing_assert_reports_the_call_site_in_full() {
    let src = "<< core.test\n^ = () -> $ => <\n  assertEq(6 * 7, 41)\n>\n";
    let (code, stderr) = run_jit("site_full", src);
    assert_eq!(code, FAIL_CODE);

    let path = tmp_dir().join("site_full.qn");
    let expected = format!(
        "{}\nassertion failed: expected 41, got 42\n  |\n3 |   assertEq(6 * 7, 41)\n  |   ^^^^^^^^^^^^^^^^^^^\n",
        position(&path, 3, 3)
    );
    assert_eq!(stderr, expected, "unexpected failure report");
}

/// Track-caller: the reported location is where the USER called the wrapper, not the
/// `failAt` hop inside `core.test`, and not the wrapper's own definition. The call sits
/// inside a helper function, so a location that merely pointed at `^` would not do.
#[test]
fn wrapper_reports_the_users_call_site_not_an_internal_hop() {
    let src = "<< core.test\n\ncheck = (n :: Num) -> $ => <\n  assertEq(n * 2, 5)\n>\n\n^ = () -> $ => <\n  check(2)\n>\n";
    let (code, stderr) = run_jit("site_wrapper", src);
    assert_eq!(code, FAIL_CODE);

    let path = tmp_dir().join("site_wrapper.qn");
    assert!(
        stderr.starts_with(&format!("{}\n", position(&path, 4, 3))),
        "must report the user's assertEq call (line 4, column 3), got: {stderr:?}"
    );
    assert!(
        stderr.contains("assertEq(n * 2, 5)"),
        "the report must show the user's own source line, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("core.test") && !stderr.contains("failAt"),
        "no internal core.test hop may appear in the report, got: {stderr:?}"
    );
}

/// The caret run covers the whole call, however wide it is — the message form's call is
/// far longer than the bare one, and both underline exactly their own text.
#[test]
fn the_caret_run_covers_the_whole_call() {
    let src =
        "<< core.test\n^ = () -> $ => <\n  assert(1 == 2, AssertOpts { message = \"boom\" })\n>\n";
    let (_, stderr) = run_jit("site_caret", src);
    let source_line = "assert(1 == 2, AssertOpts { message = \"boom\" })";
    let carets = stderr
        .lines()
        .filter_map(|l| l.rsplit_once('|').map(|(_, rest)| rest.trim()))
        .find(|rest| rest.starts_with('^'))
        .unwrap_or_default();
    assert_eq!(
        carets.len(),
        source_line.len(),
        "the caret run must be exactly as wide as the call, got: {stderr:?}"
    );
}

/// `failAt` is the reporting primitive `core.test` is built from, and it is available to
/// user code: an assertion of your own that takes a trailing `site :: Site` and forwards
/// it reports ITS caller, exactly as `assertEq` does.
#[test]
fn fail_at_reports_its_caller() {
    let src = "<< core.test\n\nassertEven = (n :: Num, site :: Site) -> $ =>\n  n % 2 == 0 ? $ : failAt(\"assertion failed: `n` is odd\", site)\n\n^ = () -> $ => <\n  assertEven(3)\n>\n";
    let (code, stderr) = run_jit("site_fail_at", src);
    assert_eq!(code, FAIL_CODE);

    let path = tmp_dir().join("site_fail_at.qn");
    assert!(
        stderr.starts_with(&format!(
            "{}\nassertion failed: 3 is odd",
            position(&path, 7, 3)
        )),
        "a custom assertion must report ITS caller (line 7), got: {stderr:?}"
    );
    assert!(
        stderr.contains("assertEven(3)"),
        "the report must underline the custom assertion's call, got: {stderr:?}"
    );
}

/// A failure inside an IMPORTED module reports that module's own path, line, and source
/// line — the location follows the span's file, not the file being compiled.
#[test]
fn a_failure_in_an_imported_module_reports_that_module() {
    let helper = write_program(
        "site_helper",
        "<< core.test\n\n>> checkDouble = (n :: Num) -> $ => <\n  assertEq(n * 2, 5)\n>\n",
    );
    let main = write_program(
        "site_importer",
        "<< \"site_helper.qn\"\n<< core.test\n\n^ = () -> $ => <\n  checkDouble(2)\n>\n",
    );
    let out = Command::new(quilon())
        .args(["run", main.to_str().unwrap()])
        .output()
        .expect("spawn quilon run");
    assert_eq!(out.status.code().unwrap_or(-1), FAIL_CODE);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!("{}\n", position(&helper, 4, 3))),
        "the report must name the imported module and its line, got: {stderr:?}"
    );
    assert!(
        stderr.contains("assertEq(n * 2, 5)"),
        "the report must show the imported module's own source line, got: {stderr:?}"
    );
}

/// Captured output is plain: color is only for a terminal, so a redirected stderr (every
/// test here, and any CI log) carries no ANSI escapes.
#[test]
fn a_redirected_report_carries_no_ansi_escapes() {
    let (_, stderr) = run_jit(
        "site_no_color",
        "<< core.test\n^ = () -> $ => assert(1 == 2)\n",
    );
    assert!(
        !stderr.contains('\u{1b}'),
        "a non-tty report must not be colored, got: {stderr:?}"
    );
}

// --- Native AOT parity -----------------------------------------------------

/// The exit-code contract must also hold for native AOT binaries (both linkers): a
/// passing assert exits 0, a failing one exits 101 with a stderr message. This is the
/// path that would expose a missing `__exit` runtime symbol (the JIT maps symbols by
/// address and could mask an AOT link failure).
#[test]
fn native_aot_assert_exit_codes() {
    let linkers: Vec<&str> = ["clang", "gcc"]
        .into_iter()
        .filter(|t| tool_available(t))
        .collect();
    if linkers.is_empty() {
        eprintln!("skipping native-AOT assert gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    }
    ensure_runtime_lib(
        Path::new(quilon())
            .parent()
            .expect("binary has a parent dir"),
    );

    for linker in &linkers {
        let (code, _) = run_aot(
            &format!("aot_pass_{linker}"),
            "<< core.test\n^ = () -> $ => assertEq(2 + 2, 4)\n",
            linker,
        );
        assert_eq!(code, 0, "native AOT ({linker}): passing assert must exit 0");

        let (code, stderr) = run_aot(
            &format!("aot_fail_{linker}"),
            "<< core.test\n^ = () -> $ => assertEq(2 + 2, 5)\n",
            linker,
        );
        assert_eq!(
            code, FAIL_CODE,
            "native AOT ({linker}): failing assert must exit {FAIL_CODE}"
        );
        assert!(
            stderr.contains("assertion failed"),
            "native AOT ({linker}): failing assert must print to stderr, got: {stderr:?}"
        );
        // The location is compiled IN, so a native binary reports it exactly as the JIT
        // does — no debug info, no unwinder, nothing to install.
        assert!(
            stderr.starts_with(&format!(
                "{}\n",
                position(&tmp_dir().join(format!("aot_fail_{linker}.qn")), 2, 16)
            )),
            "native AOT ({linker}): failing assert must report its call site, got: {stderr:?}"
        );

        // Message overload (AssertOpts) across the native path: prints opts.message.
        let (code, stderr) = run_aot(
            &format!("aot_msg_{linker}"),
            "<< core.test\n^ = () -> $ => assert(1 == 2, AssertOpts { message = \"aot boom\" })\n",
            linker,
        );
        assert_eq!(
            code, FAIL_CODE,
            "native AOT ({linker}): failing assert(cond, opts) must exit {FAIL_CODE}"
        );
        assert!(
            stderr.contains("aot boom"),
            "native AOT ({linker}): message overload must print opts.message, got: {stderr:?}"
        );
    }
}
