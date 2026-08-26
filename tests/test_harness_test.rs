//! The in-language test framework: `quilon test`, and what a top-level `describe` block
//! does to every OTHER command.
//!
//! Two halves, and both matter. A suite has to run — report each case, keep going past a
//! failure, and exit non-zero when one failed. And a suite has to COST A RELEASE BUILD
//! NOTHING: the blocks are not part of the compilation unit, so `build`/`compile`/`run`
//! neither check them nor emit them, and a file that is only tests is not a program at all.

use std::path::{Path, PathBuf};
use std::process::Command;

use quilon::ast::Item;
use quilon::lexer::Lexer;
use quilon::parser;

/// A passing suite: two groups, one nested, and a matcher for each scalar type.
const PASSING_SUITE: &str = r#"
<< core.test

describe("numbers", () => <
  it("adds", () => expect(1 + 1).toBe(2))
  it("orders", () => expect(2).toBeGreaterThan(1))

  describe("nested", () => <
    it("still runs", () => expect(true).toBeTruthy())
  >
  )
>
)

describe("text", () => <
  it("contains", () => expect("haystack").toContain("stack"))
>
)
"#;

/// One failing case between two passing ones — the shape that proves a matcher renders and
/// CONTINUES rather than exiting on the spot.
const FAILING_SUITE: &str = r#"
<< core.test

describe("arithmetic", () => <
  it("holds", () => expect(2 + 2).toBe(4))
  it("does not hold", () => expect(2 + 2).toBe(5))
  it("runs after the failure", () => expect("after").toBe("after"))
>
)
"#;

/// Where a test's `.qn` files go, unique per process so parallel runs never collide.
fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quilon_harness_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the work directory");
    dir
}

fn write(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write a test source");
    path
}

/// What a `quilon` subcommand produced.
struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn quilon(arguments: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(arguments)
        .output()
        .expect("spawn quilon");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

// ── The parse: a top-level `describe` call is not an item ───────────────────────────────

#[test]
fn a_top_level_describe_call_parses_as_a_test_block() {
    let tokens = Lexer::tokenize("<< core.test\ndescribe(\"g\", () => 0)\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    assert_eq!(program.test_blocks.len(), 1);
    assert!(
        program.items.is_empty(),
        "a test block must not also land in items: {:?}",
        program.items
    );
}

#[test]
fn a_describe_definition_is_still_an_ordinary_item() {
    // `core.test` DEFINES `describe`, and a program may define its own — only a CALL is
    // the marker, so the two are told apart by what follows the name.
    let tokens = Lexer::tokenize("describe = (n :: Num) -> Num => n\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    assert!(program.test_blocks.is_empty());
    assert!(matches!(
        program.items.as_slice(),
        [Item::FunctionDeclaration(_)]
    ));
}

// ── Stripping: what a release build does with a suite ───────────────────────────────────

#[test]
fn a_build_of_a_file_with_tests_omits_the_test_code() {
    let dir = work_dir("strip");
    let source = write(
        &dir,
        "mixed.qn",
        concat!(
            "<< core.test\n",
            "<< core.io\n",
            "double = (n :: Num) -> Num => n * 2\n",
            "describe(\"double\", () => <\n",
            "  it(\"doubles\", () => expect(double(21)).toBe(42))\n",
            ">\n",
            ")\n",
            "^ = () -> Num => double(0)\n"
        ),
    );

    let compile = quilon(&["compile", source.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("read the emitted IR");

    // The program's own function is emitted; the harness entry points are not — neither
    // `describe` itself, nor an `expect` overload, nor the summary reporter.
    assert!(
        ir.contains("@double("),
        "the program's code must be emitted"
    );
    for absent in ["@describe(", "@expect", "@reportSummary("] {
        assert!(
            !ir.contains(absent),
            "`{absent}` reached a release build:\n{ir}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_is_only_tests_is_silently_ignored() {
    let dir = work_dir("only");
    let source = write(&dir, "suite.qn", PASSING_SUITE);

    for command in ["build", "compile", "run"] {
        let out = quilon(&[command, source.to_str().unwrap()]);
        assert_eq!(
            out.code, 0,
            "`quilon {command}` on a tests-only file must succeed, said:\n{}\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "`quilon {command}` must say nothing at all, said:\n{}\n{}",
            out.stdout,
            out.stderr
        );
    }
    assert!(
        !source.with_extension("ll").exists(),
        "nothing should have been emitted for a tests-only file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_suite_may_not_define_an_entry_point() {
    let dir = work_dir("entry");
    let source = write(
        &dir,
        "both.qn",
        "<< core.test\ndescribe(\"g\", () => 0)\n^ = () -> Num => 0\n",
    );
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0, "a suite with its own `^` must be rejected");
    assert!(
        out.stderr.contains("must not define `^`"),
        "unexpected diagnostic:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Running: `quilon test` ─────────────────────────────────────────────────────────────

#[test]
fn a_passing_suite_exits_zero_and_reports_every_case() {
    let dir = work_dir("pass");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_eq!(
        out.code, 0,
        "a passing suite must exit 0:\n{}\n{}",
        out.stdout, out.stderr
    );
    for expected in ["numbers", "nested", "text", "adds", "contains"] {
        assert!(
            out.stdout.contains(expected),
            "`{expected}` is missing from the report:\n{}",
            out.stdout
        );
    }
    assert!(
        out.stdout.contains("4 cases, 4 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_case_exits_non_zero_without_stopping_the_run() {
    let dir = work_dir("fail");
    let source = write(&dir, "suite.qn", FAILING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_ne!(out.code, 0, "a failing suite must exit non-zero");
    assert!(
        out.stdout.contains("3 cases, 2 passed, 1 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    // The case AFTER the failure ran, which is the render-and-continue contract.
    assert!(
        out.stdout.contains("runs after the failure"),
        "the run stopped at the first failure:\n{}",
        out.stdout
    );
    // The failure itself is reported in the compiler's own diagnostic format, on stderr,
    // blaming the `expect(…)` that was written.
    assert!(
        out.stderr.contains("suite.qn:6:"),
        "the failure must name file:line:column:\n{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("expected 5, got 4") && out.stderr.contains("^^^"),
        "the failure must carry the message and a caret run:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_runs_every_suite_it_holds() {
    let dir = work_dir("dir");
    write(&dir, "green.qn", PASSING_SUITE);
    write(&dir, "red.qn", FAILING_SUITE);
    // Not a suite: a program with no test blocks is passed over, not run.
    write(&dir, "program.qn", "^ = () -> Num => 7\n");

    let out = quilon(&["test", dir.to_str().unwrap()]);
    assert_ne!(out.code, 0, "one suite failed, so the run failed");
    assert!(
        out.stdout.contains("green.qn") && out.stdout.contains("red.qn"),
        "both suites should have run:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("program.qn"),
        "a file with no test blocks is not a suite:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("2 suites: 1 passed, 1 failed"),
        "unexpected per-file tally:\n{}",
        out.stdout
    );

    // Each suite's totals are its own — the registry is per-thread, and a thread per file
    // is what keeps them apart.
    assert!(
        out.stdout.contains("4 cases, 4 passed, 0 failed")
            && out.stdout.contains("3 cases, 2 passed, 1 failed"),
        "one suite's totals leaked into another's summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_with_no_suites_succeeds_and_says_so() {
    let dir = work_dir("empty");
    write(&dir, "program.qn", "^ = () -> Num => 0\n");
    let out = quilon(&["test", dir.to_str().unwrap()]);
    assert_eq!(out.code, 0, "nothing failed, so nothing is wrong");
    assert!(
        out.stdout.contains("no tests found"),
        "unexpected output:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_suite_that_does_not_compile_fails_the_run() {
    let dir = work_dir("broken");
    let source = write(
        &dir,
        "suite.qn",
        "<< core.test\ndescribe(\"g\", () => expect(1).toBe(\"one\"))\n",
    );
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0, "a suite that fails to type-check must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A suite with no `<< core.test` has no `describe` to call, and says so where the call is
/// — rather than blaming the entry point the compiler synthesized.
#[test]
fn a_suite_without_the_import_is_reported_at_its_own_describe() {
    let dir = work_dir("noimport");
    let source = write(&dir, "suite.qn", "describe(\"g\", () => 0)\n");
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("suite.qn:1:"),
        "the diagnostic must point at the `describe` call:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The shipped example is the documented demonstration of the framework, so it is a gate.
#[test]
fn the_example_suite_passes() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/test_suite.qn");
    let out = quilon(&["test", example.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "examples/test_suite.qn must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains("0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
}
