//! The in-language test framework: `quilon test`, and what a top-level `describe` block does
//! to every other command.
//!
//! Two halves, and both matter. A suite has to run — report each case, and exit non-zero
//! when one failed. And a suite has to cost a release build nothing: the blocks are not part
//! of the compilation unit, so `run`/`compile`/`build` neither check them nor emit them, and
//! a file that is only tests is not a program at all.

use std::path::{Path, PathBuf};
use std::process::Command;

use quilon::ast::Item;
use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{ensure_runtime_lib, tool_available};

/// A passing suite: two groups, one nested inside the other.
const PASSING_SUITE: &str = r#"
<< core.test

describe("numbers", () => <
  it("adds", () => assertEq(1 + 1, 2))
  it("orders", () => assert(2 > 1))

  describe("nested", () => <
    it("still runs", () => assert(true))
  >
  )
>
)

describe("text", () => <
  it("contains", () => assert("haystack".contains("stack")))
>
)
"#;

/// A passing case, then a failing one, then another that would pass. The assertions are
/// fail-fast, so the third never runs — which is what the report has to reflect.
const FAILING_SUITE: &str = r#"
<< core.test

describe("arithmetic", () => <
  it("holds", () => assertEq(2 + 2, 4))
  it("does not hold", () => assertEq(2 + 2, 5))
  it("never reached", () => assertEq("after", "after"))
>
)
"#;

/// The line `examples/tests_alongside_code.qn` prints from inside a `describe` block. Nothing
/// else in the repository prints it, so finding it in a build's output means the block ran.
const STRIPPED_BLOCK_MARKER: &str = "STRIPPED-TEST-BLOCK-RAN";

/// The line `examples/use_tested_module.qn` prints from its `^`. Its presence is what tells a
/// build that erased the test blocks apart from a build that never ran at all.
const PROGRAM_MARKER: &str = "PROGRAM-RAN-WITHOUT-TEST-BLOCKS";

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

/// A file shipped in `examples/`.
fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

/// The first linker available on PATH, or `None` on a box that has neither — which is where
/// the native half of a gate skips instead of failing.
fn available_linker() -> Option<&'static str> {
    ["clang", "gcc"]
        .into_iter()
        .find(|tool| tool_available(tool))
}

/// `quilon build` `source` into `directory` with `linker`, then execute what it produced.
fn build_and_execute(source: &Path, directory: &Path, linker: &str) -> Output {
    ensure_runtime_lib(
        Path::new(env!("CARGO_BIN_EXE_quilon"))
            .parent()
            .expect("the compiler's directory"),
    );
    let binary = directory.join("built");
    let build = quilon(&[
        "build",
        source.to_str().unwrap(),
        "--linker",
        linker,
        "-o",
        binary.to_str().unwrap(),
    ]);
    assert_eq!(
        build.code, 0,
        "`quilon build --linker {linker}` failed:\n{}",
        build.stderr
    );

    let run = Command::new(&binary)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the built executable");
    Output {
        code: run.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
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
            "  it(\"doubles\", () => assertEq(double(21), 42))\n",
            ">\n",
            ")\n",
            "^ = () -> Num => double(0)\n"
        ),
    );

    let compile = quilon(&["compile", source.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("read the emitted IR");

    // The program's own function is emitted, and NOTHING of the harness: the blocks were
    // never checked or lowered, so nothing reaches `describe`, `it`, or the reporter, and
    // reachability pruning drops all three.
    let defined: Vec<&str> = ir
        .lines()
        .filter(|line| line.starts_with("define"))
        .collect();
    assert!(
        defined.iter().any(|line| line.contains("@double(")),
        "the program's code must be emitted:\n{defined:#?}"
    );
    for absent in ["describe", "@it(", "report", "indent", "green"] {
        assert!(
            !defined.iter().any(|line| line.contains(absent)),
            "`{absent}` reached a release build:\n{defined:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A program with its tests beside it, as source: an `^` that prints [`PROGRAM_MARKER`], and a
/// `describe` block over the same function that prints [`STRIPPED_BLOCK_MARKER`]. Which line
/// comes out says which halves of the file were compiled.
fn program_with_tests_beside_it() -> String {
    format!(
        r#"
<< core.io
<< core.test

double = (n :: Num) -> Num => n * 2

describe("double", () => <
  it("doubles", () => <
    print("{STRIPPED_BLOCK_MARKER}")
    assertEq(double(21), 42)
  >
  )
>
)

^ = () -> $ => <
  print("{PROGRAM_MARKER}")
  assertEq(double(4), 8)
>
"#
    )
}

/// The whole point, in three assertions: `out` is a run of a program whose file also holds test
/// blocks, so it exits 0 having printed [`PROGRAM_MARKER`] — which tells "the blocks were
/// erased" apart from "nothing ran at all" — and no [`STRIPPED_BLOCK_MARKER`].
fn assert_ran_without_its_tests(what: &str, out: &Output) {
    assert_eq!(
        out.code, 0,
        "{what} must exit 0:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains(PROGRAM_MARKER),
        "{what} printed nothing of its own, so nothing ran:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains(STRIPPED_BLOCK_MARKER),
        "{what} ran a test block that should have been stripped:\n{}",
        out.stdout
    );
}

/// The stripping, observed rather than inferred: the `describe` block's `print` never executes
/// in a build of the file it sits in — under the JIT and a native build alike.
#[test]
fn a_describe_block_beside_an_entry_point_never_runs_in_a_build() {
    let dir = work_dir("beside");
    let source = write(&dir, "program.qn", &program_with_tests_beside_it());

    let run = quilon(&["run", source.to_str().unwrap()]);
    assert_ran_without_its_tests("`quilon run`", &run);

    match available_linker() {
        Some(linker) => {
            let native = build_and_execute(&source, &dir, linker);
            assert_ran_without_its_tests("the built program", &native);
        }
        None => eprintln!("skipping the native half: need a linker (`clang` or `gcc`) on PATH"),
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
        out.stdout.contains("4 cases passed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_case_exits_non_zero_and_ends_the_run_where_it_failed() {
    let dir = work_dir("fail");
    let source = write(&dir, "suite.qn", FAILING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_ne!(out.code, 0, "a failing suite must exit non-zero");
    // Cases before the failure are reported; the failing one and everything after it are
    // not, the assertions being fail-fast. And no summary: the run never got there.
    assert!(
        out.stdout.contains("holds"),
        "the case before the failure should have been reported:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("never reached") && !out.stdout.contains("cases passed"),
        "a fail-fast run must not report past the failure:\n{}",
        out.stdout
    );
    // The failure itself is reported in the compiler's own diagnostic format, on stderr,
    // blaming the assertion that was written.
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

    // The passing suite's total is its own — a process per suite is what keeps one suite's
    // counts out of another's summary. The failing suite ran one case before it failed, and
    // that count must not have landed here.
    assert!(
        out.stdout.contains("4 cases passed"),
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
        "<< core.test\ndescribe(\"g\", () => assertEq(1, \"one\"))\n",
    );
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0, "a suite that fails to type-check must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A suite with no `<< core.test` has no reporter, and says so at its own `describe` —
/// rather than blaming the entry point the compiler synthesized, which has no location.
#[test]
fn a_suite_without_a_reporter_is_reported_at_its_own_describe() {
    let dir = work_dir("noimport");
    let source = write(&dir, "suite.qn", "\ndescribe(\"g\", () => 0)\n");
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("suite.qn:2:") && out.stderr.contains("no test reporter"),
        "the diagnostic must point at the `describe` call:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mistyped path in a CI invocation must not report success.
#[test]
fn a_path_that_does_not_exist_fails_the_run() {
    let out = quilon(&["test", "no/such/directory"]);
    assert_ne!(out.code, 0, "a missing path must fail:\n{}", out.stdout);
    assert!(
        out.stderr.contains("no such file or directory"),
        "unexpected diagnostic:\n{}",
        out.stderr
    );
}

/// The failure that would be silent: a suite whose syntax is broken. It has no parseable
/// `describe` to be recognized by, so passing over it would report success on a suite
/// somebody had just broken.
#[test]
fn a_suite_that_does_not_parse_fails_the_run() {
    let dir = work_dir("unparseable");
    write(
        &dir,
        "suite.qn",
        "<< core.test\ndescribe(\"g\", () => <<<\n",
    );
    let out = quilon(&["test", dir.to_str().unwrap()]);
    assert_ne!(
        out.code, 0,
        "an unparseable suite must fail, not vanish:\n{}\n{}",
        out.stdout, out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A suite may carry its own fixtures — helper functions, record types — and is still not a
/// program: stripping its blocks leaves nothing to run, so `build` passes over it.
#[test]
fn a_suite_with_its_own_helpers_is_still_not_a_program() {
    let dir = work_dir("helpers");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.test\n",
            "double = (n :: Num) -> Num => n * 2\n",
            "describe(\"double\", () => <\n",
            "  it(\"doubles\", () => assertEq(double(21), 42))\n",
            ">\n",
            ")\n"
        ),
    );

    let build = quilon(&["build", source.to_str().unwrap()]);
    assert_eq!(
        build.code, 0,
        "`quilon build` must pass over a suite with helpers:\n{}\n{}",
        build.stdout, build.stderr
    );
    assert!(
        build.stdout.is_empty() && build.stderr.is_empty(),
        "the skip must be silent, said:\n{}\n{}",
        build.stdout,
        build.stderr
    );

    let test = quilon(&["test", source.to_str().unwrap()]);
    assert_eq!(
        test.code, 0,
        "the same file must run as a suite:\n{}\n{}",
        test.stdout, test.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The shipped example is the documented demonstration of the framework, so it is a gate.
#[test]
fn the_example_suite_passes() {
    let suite = example("test_suite.qn");
    let out = quilon(&["test", suite.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "examples/test_suite.qn must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains("cases passed"),
        "unexpected summary:\n{}",
        out.stdout
    );
}

// ── The shipped pair: tests beside the code, and what each command does with them ────────

/// The shipped pair, one command each way: `examples/tests_alongside_code.qn` keeps its
/// `describe` blocks beside the exports they check, and neither running the module nor building
/// a program out of its exports produces a word of test output — under the JIT and a native
/// build alike. (Same-file stripping is the sibling test above; what this adds is the import
/// boundary, where a module's blocks are dropped rather than stripped.)
#[test]
fn the_shipped_module_keeps_its_tests_out_of_a_program_that_imports_it() {
    let module = example("tests_alongside_code.qn");
    let program = example("use_tested_module.qn");

    // The module is a suite, so `run` has nothing to run and says so by saying nothing.
    let module_run = quilon(&["run", module.to_str().unwrap()]);
    assert!(
        module_run.stdout.is_empty(),
        "`quilon run` on a suite must print nothing:\n{}",
        module_run.stdout
    );

    let program_run = quilon(&["run", program.to_str().unwrap()]);
    assert_ran_without_its_tests("`quilon run` on the program", &program_run);

    match available_linker() {
        Some(linker) => {
            let dir = work_dir("shipped");
            let native = build_and_execute(&program, &dir, linker);
            assert_ran_without_its_tests("the built program", &native);
            let _ = std::fs::remove_dir_all(&dir);
        }
        None => eprintln!("skipping the native half: need a linker (`clang` or `gcc`) on PATH"),
    }
}

/// The other direction, on the very file the build erased: `quilon test` compiles those blocks,
/// so the line no build ever showed is on stdout and its case is reported.
#[test]
fn the_shipped_example_runs_its_tests_under_quilon_test() {
    let module = example("tests_alongside_code.qn");
    let out = quilon(&["test", module.to_str().unwrap()]);

    assert_eq!(
        out.code, 0,
        "the shipped suite must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains(STRIPPED_BLOCK_MARKER),
        "the test block did not run under `quilon test`:\n{}",
        out.stdout
    );
    for expected in [
        "slugify",
        "wordCount",
        "the stripped block",
        "4 cases passed",
    ] {
        assert!(
            out.stdout.contains(expected),
            "`{expected}` is missing from the report:\n{}",
            out.stdout
        );
    }
}
