//! The provided assertions: `assert(actual, matcher)` end-to-end.
//!
//! A holding assertion does nothing; a FAILING one prints a located report to stderr and
//! exits **101** (the Rust-panic convention, distinct from the small result codes examples
//! use as their normal exit status).
//!
//! The report names the failing call's own `file:line:column` and underlines it, wherever the
//! call sits — inside a helper, or inside an imported module. Those are the cases under
//! "Call-site reporting" below. `expect`, the recorded half of the same vocabulary, is
//! covered by `tests/test_harness_test.rs`, where there is a run to record into.
//!
//! Every case is driven as a SUBPROCESS — `quilon run <file>` (in-process JIT) and, where a
//! linker is present, `quilon build` + execute (native AOT). It must never be the in-process
//! `jit::run_program`, because a failing assertion terminates the process it runs in, which
//! would take the test runner with it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::{ensure_runtime_lib, frame, position};

const FAIL_CODE: i32 = 101;

fn quilon() -> &'static str {
    env!("CARGO_BIN_EXE_quilon")
}

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quilon_assert_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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

/// `quilon run` on an entry point whose body is `body` — the shape most cases here need.
fn run_entry(tag: &str, body: &str) -> (i32, String) {
    run_jit(tag, &format!("^ = () -> $ => < {body} >\n"))
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

// --- The form itself ------------------------------------------------------

/// `assert` and the matchers are the COMPILER's, like `print` — a program reaches them with
/// no import at all.
#[test]
fn assert_needs_no_import() {
    let (code, stderr) = run_entry("no_import", "assert(1 + 1, equals(2))");
    assert_eq!(code, 0, "unexpected failure: {stderr}");
}

/// An assertion takes a value AND a matcher. A bare condition, or a second argument that is
/// not one of the provided matchers, names the vocabulary instead of resolving to something
/// surprising.
#[test]
fn an_assertion_without_a_matcher_names_the_vocabulary() {
    for (tag, body) in [
        ("bare_condition", "assert(1 + 1 == 2)"),
        ("not_a_matcher", "assert(1 + 1, true)"),
        ("a_call_that_is_not_a_matcher", "assert(1 + 1, equal(2))"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_ne!(code, 0, "`{body}` must be refused");
        assert!(
            stderr.contains("takes the value and a matcher") && stderr.contains("`equals`"),
            "the diagnostic for `{body}` must name the vocabulary, got: {stderr:?}"
        );
    }
}

// --- equals ---------------------------------------------------------------

#[test]
fn equals_holds_and_fails_over_every_built_in() {
    for (tag, body) in [
        ("eq_num", "assert(6 * 7, equals(42))"),
        ("eq_text", "assert(\"a\" + \"b\", equals(\"ab\"))"),
        ("eq_bool", "assert(1 < 2, equals(true))"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_eq!(code, 0, "`{body}` must hold, got: {stderr:?}");
    }

    let (code, stderr) = run_entry("eq_num_fail", "assert(6 * 7, equals(41))");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("assertion failed: expected 41, got 42"),
        "the report must name expected and actual, got: {stderr:?}"
    );
}

/// A `Text` in a report is QUOTED, so a trailing space or an empty string is visible rather
/// than lost in the sentence around it.
#[test]
fn a_text_value_is_quoted_in_the_report() {
    let (code, stderr) = run_entry("eq_text_fail", "assert(\"x \", equals(\"x\"))");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected \"x\", got \"x \""),
        "a Text must be quoted in the report, got: {stderr:?}"
    );
}

/// `equals` compares through the `==` MEMBER, so a user record works exactly as far as its
/// own `==` does — and renders through its own `` ` ``.
#[test]
fn equals_uses_a_user_records_own_equality_and_rendering() {
    let record = concat!(
        "Version = {\n",
        "  major :: Num,\n",
        "  minor :: Num,\n",
        "  == = (other :: Version) -> Bool => < it.major == other.major && it.minor == other.minor >,\n",
        "  ` = => < \"v`it.major`.`it.minor`\" >\n",
        "}\n"
    );
    let (code, stderr) = run_jit(
        "eq_record_pass",
        &format!(
            "{record}^ = () -> $ => < assert(Version {{ major = 0, minor = 9 }}, equals(Version {{ major = 0, minor = 9 }})) >\n"
        ),
    );
    assert_eq!(code, 0, "equal records must hold, got: {stderr:?}");

    let (code, stderr) = run_jit(
        "eq_record_fail",
        &format!(
            "{record}^ = () -> $ => < assert(Version {{ major = 0, minor = 9 }}, equals(Version {{ major = 1, minor = 0 }})) >\n"
        ),
    );
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected v1.0, got v0.9"),
        "both values must render through the type's own `` ` ``, got: {stderr:?}"
    );
}

/// The same for a user SUM: its `==` member decides, and its `` ` `` renders.
#[test]
fn equals_uses_a_user_sums_own_equality_and_rendering() {
    let sum = concat!(
        "Light = Red / Green {\n",
        "  ` = => < it ? | Red => \"Red\" | Green => \"Green\" >,\n",
        "  == = (other :: Light) -> Bool => < \"`it`\" == \"`other`\" >\n",
        "}\n"
    );
    let (code, stderr) = run_jit(
        "eq_sum_pass",
        &format!("{sum}^ = () -> $ => < assert(Red, equals(Red)) >\n"),
    );
    assert_eq!(code, 0, "equal sum values must hold, got: {stderr:?}");

    let (code, stderr) = run_jit(
        "eq_sum_fail",
        &format!("{sum}^ = () -> $ => < assert(Red, equals(Green)) >\n"),
    );
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected Green, got Red"),
        "the sum's own rendering must appear in the report, got: {stderr:?}"
    );
}

/// A type with no `==` member cannot be compared, and the diagnostic says which member is
/// missing rather than failing somewhere in codegen.
#[test]
fn equals_on_a_type_without_equality_is_refused() {
    let (code, stderr) = run_jit(
        "eq_no_member",
        "P = { x :: Num }\n^ = () -> $ => < assert(P { x = 1 }, equals(P { x = 1 })) >\n",
    );
    assert_ne!(code, 0);
    assert!(
        stderr.contains("compares with `==`") && stderr.contains("P"),
        "unexpected diagnostic: {stderr:?}"
    );
}

/// A sum payload whose type is not yet concrete is REPRESENTED as a `Num`, while the checker
/// treats it as compatible with anything — so pairing one with a `Text` or a `Bool` has to be
/// refused here, or codegen would be handed two different representations to compare.
#[test]
fn a_generic_payload_is_only_compared_against_a_num() {
    let program = "f = (n :: Num) -> Result => < n > 0 ? Ok(n) : NotOk($) >\n\
                   ^ = () -> $ => < f(1) ? | Ok(v) => assert(v, MATCHER) | NotOk(_) => $ >\n";
    for (tag, matcher) in [
        ("generic_text", "equals(\"x\")"),
        ("generic_bool", "equals(true)"),
        ("generic_contains", "contains(\"x\")"),
    ] {
        let (code, stderr) = run_jit(tag, &program.replace("MATCHER", matcher));
        assert_ne!(code, 0, "`{matcher}` on a generic payload must be refused");
        assert!(
            stderr.contains("error[Q"),
            "`{matcher}` must be refused with a diagnostic, got: {stderr:?}"
        );
    }

    // Against a `Num` it is exactly the comparison it looks like.
    let (code, stderr) = run_jit("generic_num", &program.replace("MATCHER", "equals(1)"));
    assert_eq!(code, 0, "unexpected failure: {stderr:?}");
}

// --- contains -------------------------------------------------------------

#[test]
fn contains_reads_a_text_and_an_array() {
    for (tag, body) in [
        ("has_text", "assert(\"haystack\", contains(\"stack\"))"),
        ("has_elem", "assert([2, 4, 6], contains(4))"),
        ("has_text_elem", "assert([\"a\", \"b\"], contains(\"b\"))"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_eq!(code, 0, "`{body}` must hold, got: {stderr:?}");
    }

    let (code, stderr) = run_entry(
        "has_text_fail",
        "assert(\"haystack\", contains(\"needle\"))",
    );
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected something containing \"needle\", got \"haystack\""),
        "unexpected report: {stderr:?}"
    );

    let (code, stderr) = run_entry("has_elem_fail", "assert([2, 4, 6], contains(5))");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected something containing 5, got [2, 4, 6]"),
        "unexpected report: {stderr:?}"
    );
}

/// An array of a user record scans with the element type's own `==`.
#[test]
fn contains_scans_an_array_of_records_with_their_equality() {
    let record = concat!(
        "Tag = {\n",
        "  name :: Text,\n",
        "  == = (other :: Tag) -> Bool => < it.name == other.name >,\n",
        "  ` = => < it.name >\n",
        "}\n"
    );
    let (code, stderr) = run_jit(
        "has_record",
        &format!(
            "{record}^ = () -> $ => < assert([Tag {{ name = \"a\" }}, Tag {{ name = \"b\" }}], contains(Tag {{ name = \"b\" }})) >\n"
        ),
    );
    assert_eq!(code, 0, "unexpected failure: {stderr:?}");
}

#[test]
fn contains_on_a_type_it_cannot_read_is_refused() {
    let (code, stderr) = run_entry("has_num", "assert(42, contains(4))");
    assert_ne!(code, 0);
    assert!(
        stderr.contains("reads a `Text` or an array"),
        "unexpected diagnostic: {stderr:?}"
    );
}

// --- not ------------------------------------------------------------------

#[test]
fn not_negates_any_matcher() {
    for (tag, body) in [
        ("not_eq", "assert(1, not(equals(2)))"),
        ("not_text", "assert(\"a\", not(equals(\"b\")))"),
        ("not_has", "assert([2, 4], not(contains(5)))"),
        ("not_ok", "assert([1].at(9), not(isOk()))"),
        ("not_not", "assert(1, not(not(equals(1))))"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_eq!(code, 0, "`{body}` must hold, got: {stderr:?}");
    }

    let (code, stderr) = run_entry("not_fail", "assert(5, not(equals(5)))");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected not 5, got 5"),
        "a negation must say so in the report, got: {stderr:?}"
    );
}

// --- isOk / isNotOk -------------------------------------------------------

#[test]
fn is_ok_and_is_not_ok_read_a_result() {
    for (tag, body) in [
        ("ok_pass", "assert([10, 20].at(0), isOk())"),
        ("notok_pass", "assert([10, 20].at(9), isNotOk())"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_eq!(code, 0, "`{body}` must hold, got: {stderr:?}");
    }

    let (code, stderr) = run_entry("ok_fail", "assert([10, 20].at(9), isOk())");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected Ok, got NotOk"),
        "unexpected report: {stderr:?}"
    );

    let (code, stderr) = run_entry("notok_fail", "assert([10, 20].at(0), isNotOk())");
    assert_eq!(code, FAIL_CODE);
    assert!(
        stderr.contains("expected NotOk, got Ok"),
        "unexpected report: {stderr:?}"
    );
}

/// A sum with neither variant is refused at compile time, rather than comparing against a
/// tag it does not have.
#[test]
fn is_ok_on_a_sum_without_that_variant_is_refused() {
    let (code, stderr) = run_jit(
        "isok_wrong_sum",
        "Light = Red / Green\n^ = () -> $ => < assert(Red, isOk()) >\n",
    );
    assert_ne!(code, 0);
    assert!(
        stderr.contains("reads a `Result`"),
        "unexpected diagnostic: {stderr:?}"
    );
}

/// A matcher's arity is checked: `isOk` takes nothing, `equals` takes one value.
#[test]
fn a_matcher_with_the_wrong_number_of_arguments_is_refused() {
    for (tag, body) in [
        ("arity_ok", "assert([1].at(0), isOk(1))"),
        ("arity_equals", "assert(1, equals(1, 2))"),
    ] {
        let (code, stderr) = run_entry(tag, body);
        assert_ne!(code, 0, "`{body}` must be refused");
        assert!(
            stderr.contains("argument(s)"),
            "unexpected diagnostic for `{body}`: {stderr:?}"
        );
    }
}

// --- Call-site reporting ---------------------------------------------------

/// The whole report for one known failure, line for line: position, message, gutter,
/// source line, caret run. Pinning the exact shape is the point — this is the output a
/// person reads when a check fails, and it must stay a compiler-style diagnostic.
#[test]
fn a_failing_assert_reports_the_call_site_in_full() {
    let src = "^ = () -> $ => <\n  assert(6 * 7, equals(41))\n>\n";
    let (code, stderr) = run_jit("site_full", src);
    assert_eq!(code, FAIL_CODE);

    let path = tmp_dir().join("site_full.qn");
    let expected = format!(
        "error[Q069]: assertion failed: expected 41, got 42\n{}\n",
        frame(
            &position(&path, 2, 3),
            2,
            3,
            "  assert(6 * 7, equals(41))",
            "assert(6 * 7, equals(41))".len()
        )
    );
    assert_eq!(stderr, expected, "unexpected failure report");
}

/// The location is the assertion's own line, wherever it sits — here inside a helper, so a
/// report that merely pointed at `^` would not do.
#[test]
fn an_assert_inside_a_helper_reports_that_helper_line() {
    let src = "check = (n :: Num) -> $ => <\n  assert(n * 2, equals(5))\n>\n\n^ = () -> $ => <\n  check(2)\n>\n";
    let (code, stderr) = run_jit("site_helper_line", src);
    assert_eq!(code, FAIL_CODE);

    let path = tmp_dir().join("site_helper_line.qn");
    assert!(
        stderr.contains(&position(&path, 2, 3)),
        "must report the assertion's own line 2, column 3, got: {stderr:?}"
    );
    assert!(
        stderr.contains("assert(n * 2, equals(5))"),
        "the report must show that source line, got: {stderr:?}"
    );
}

/// `failAt` is the reporting primitive `core.test` exports for a check of your own: take a
/// trailing `site :: Site` and forward it, and the report blames ITS caller.
#[test]
fn fail_at_reports_its_caller() {
    let src = "<< core.test\n\nassertEven = (n :: Num, site :: Site) -> $ => <\n  n % 2 == 0 ? $ : test.failAt(\"assertion failed: `n` is odd\", site)\n>\n^ = () -> $ => <\n  assertEven(3)\n>\n";
    let (code, stderr) = run_jit("site_fail_at", src);
    assert_eq!(code, FAIL_CODE);

    // `failAt` composes its frame in Quilon (`corelib/test.qn`): the position line, then
    // the message.
    let path = tmp_dir().join("site_fail_at.qn");
    assert!(
        stderr.starts_with(&format!(
            "{}:7:3:\nassertion failed: 3 is odd",
            quilon::source_map::shorten_path(&path.display().to_string())
        )),
        "a check of your own must report ITS caller (line 7), got: {stderr:?}"
    );
    assert!(
        stderr.contains("assertEven(3)"),
        "the report must underline that call, got: {stderr:?}"
    );
}

/// A failure inside an IMPORTED module reports that module's own path, line, and source
/// line — the location follows the span's file, not the file being compiled.
#[test]
fn a_failure_in_an_imported_module_reports_that_module() {
    let helper = write_program(
        "site_helper",
        ">> checkDouble = (n :: Num) -> $ => <\n  assert(n * 2, equals(5))\n>\n",
    );
    let main = write_program(
        "site_importer",
        "<< \"site_helper.qn\"\n\n^ = () -> $ => <\n  site_helper.checkDouble(2)\n>\n",
    );
    let out = Command::new(quilon())
        .args(["run", main.to_str().unwrap()])
        .output()
        .expect("spawn quilon run");
    assert_eq!(out.status.code().unwrap_or(-1), FAIL_CODE);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&position(&helper, 2, 3)),
        "the report must name the imported module and its line, got: {stderr:?}"
    );
    assert!(
        stderr.contains("assert(n * 2, equals(5))"),
        "the report must show the imported module's own source line, got: {stderr:?}"
    );
}

/// Captured output is plain: color is only for a terminal, so a redirected stderr (every
/// test here, and any CI log) carries no ANSI escapes.
#[test]
fn a_redirected_report_carries_no_ansi_escapes() {
    let (_, stderr) = run_entry("site_no_color", "assert(1, equals(2))");
    assert!(
        !stderr.contains('\u{1b}'),
        "a non-tty report must not be colored, got: {stderr:?}"
    );
}

/// The value under test is evaluated ONCE, however the assertion reports it: the condition
/// and the message read the same value, so a side-effecting subject cannot run twice.
#[test]
fn the_value_under_test_is_evaluated_once() {
    let src = concat!(
        "<< core.io\n",
        "^ = () -> $ => <\n",
        "  assert([1, 2].each((n :: Num) => io.print(n)), contains(1))\n",
        ">\n"
    );
    let path = write_program("evaluated_once", src);
    let out = Command::new(quilon())
        .args(["run", path.to_str().unwrap()])
        .output()
        .expect("spawn quilon run");
    assert_eq!(out.status.code().unwrap_or(-1), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.matches('1').count(),
        1,
        "the subject ran more than once: {stdout:?}"
    );
}

// --- Native AOT parity -----------------------------------------------------

/// The exit-code contract must also hold for native AOT binaries (both linkers): a holding
/// assertion exits 0, a failing one exits 101 with a located stderr report. This is the path
/// that would expose a missing runtime symbol (the JIT maps symbols by address and could mask
/// an AOT link failure).
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
            "^ = () -> $ => < assert(2 + 2, equals(4)) >\n",
            linker,
        );
        assert_eq!(code, 0, "native AOT ({linker}): a holding assert exits 0");

        let (code, stderr) = run_aot(
            &format!("aot_fail_{linker}"),
            "^ = () -> $ => < assert(2 + 2, equals(5)) >\n",
            linker,
        );
        assert_eq!(
            code, FAIL_CODE,
            "native AOT ({linker}): a failing assert must exit {FAIL_CODE}"
        );
        assert!(
            stderr.contains("assertion failed: expected 5, got 4"),
            "native AOT ({linker}): the report must carry the message, got: {stderr:?}"
        );
        // The location is compiled IN, so a native binary reports it exactly as the JIT
        // does — no debug info, no unwinder, nothing to install.
        assert!(
            stderr.contains(&position(
                &tmp_dir().join(format!("aot_fail_{linker}.qn")),
                1,
                18
            )),
            "native AOT ({linker}): the report must name its call site, got: {stderr:?}"
        );
    }
}
