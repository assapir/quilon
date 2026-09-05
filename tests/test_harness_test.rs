//! The in-language test framework: `quilon test`, and what a top-level `describe` block does
//! to every other command.
//!
//! Two halves, and both matter. A suite has to run — report each case whichever way it went,
//! tally them, and exit non-zero when one failed. And a suite has to cost a release build
//! nothing: the blocks are not part of the compilation unit, so `run`/`compile`/`build`
//! neither check them nor emit them, and a file that is only tests is not a program at all.
//! Both halves apply to ONE file: tests may sit beside the code they test, `^` included,
//! which is where the two meet.

use std::path::{Path, PathBuf};
use std::process::Command;

use quilon::ast::Item;
use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{ensure_runtime_lib, tool_available};

/// Run the executable at `path` with no arguments, capturing what it produced.
fn execute(path: &Path) -> Output {
    let out = Command::new(path)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the built executable");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A passing suite: two groups, one nested inside the other.
const PASSING_SUITE: &str = r#"
<< core.test

test.describe("numbers", () => <
  test.it("adds", () => expect(1 + 1, equals(2)))
  test.it("orders", () => expect(2 > 1, equals(true)))

  test.describe("nested", () => <
    test.it("still runs", () => expect(true, equals(true)))
  >)
>)

test.describe("text", () => <
  test.it("contains", () => expect("haystack", contains("stack")))
>)
"#;

/// A passing case, then a failing one, then another that passes. A failed `expect` marks its
/// own case and nothing else, so the third case still runs and the summary tallies both ways.
const FAILING_SUITE: &str = r#"
<< core.test

test.describe("arithmetic", () => <
  test.it("holds", () => expect(2 + 2, equals(4)))
  test.it("does not hold", () => expect(2 + 2, equals(5)))
  test.it("runs after the failure", () => expect("after", equals("after")))
>)
"#;

/// The line a `describe` block prints — in `examples/tests_alongside_code.qn` and in this
/// file's fixtures. Nothing else in the repository prints it, so finding it in a build's
/// output means a block that should have been erased ran.
const ERASED_BLOCK_MARKER: &str = "ERASED-TEST-BLOCK-RAN";

/// The line a program prints from its own `^`, beside its test blocks. Its presence is what
/// tells a build that erased those blocks apart from a build that never ran at all — and its
/// ABSENCE under `quilon test` is what proves that `^` is not the test run's entry point.
const PROGRAM_MARKER: &str = "PROGRAM-RAN-WITHOUT-ITS-TEST-BLOCKS";

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
    let tokens = Lexer::tokenize("<< core.test\ntest.describe(\"g\", () => 0)\n").expect("lexing");
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
    // `core.test` DEFINES `describe`, and a program may define its own — only a CALL is the
    // marker, so the two are told apart by what follows the name.
    let tokens = Lexer::tokenize("describe = (n :: Num) -> Num => < n >\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    assert!(program.test_blocks.is_empty());
    assert!(matches!(
        program.items.as_slice(),
        [Item::FunctionDeclaration(_)]
    ));
}

// ── Erasure: what a release build does with a suite ─────────────────────────────────────

/// What `core.test` defines, none of which may reach a build: the harness, the summary it
/// ends with, and the state and lifecycle behind them.
const HARNESS_SYMBOLS: [&str; 11] = [
    "describe",
    "core.test.it",
    "report",
    "failAt",
    "enterSuite",
    "leaveSuite",
    "casesPassed",
    "casesFailed",
    "nestingDepth",
    "caseFailing",
    "finishCase",
];

/// The `define` lines of an emitted `.ll` — what a build kept, by name and by count.
fn defined_functions(ir: &str) -> Vec<&str> {
    ir.lines()
        .filter(|line| line.starts_with("define"))
        .collect()
}

/// Assert that `ir` defines none of [`HARNESS_SYMBOLS`]; `what` names the build for the report.
fn assert_no_harness_emitted(ir: &str, what: &str) {
    let defined = defined_functions(ir);
    for absent in HARNESS_SYMBOLS {
        assert!(
            !defined.iter().any(|line| line.contains(absent)),
            "`{absent}` reached {what}:\n{defined:#?}"
        );
    }
}

#[test]
fn a_build_of_a_file_with_tests_omits_the_test_code() {
    let dir = work_dir("erase");
    // The record with methods is load-bearing: a method body mentions `it`, its receiver, and
    // a pass that reads that as a mention of the harness's top-level `it` keeps the whole
    // harness alive. A fixture with no methods never asks the question.
    let source = write(
        &dir,
        "mixed.qn",
        concat!(
            "<< core.test\n",
            "<< core.io\n",
            "double = (n :: Num) -> Num => < n * 2 >\n",
            "Counter = {\n",
            "  total :: Num,\n",
            "  bumped = => < it.total + 1 >,\n",
            "  ` = => < \"counter\" >\n",
            "}\n",
            "test.describe(\"double\", () => <\n",
            "  test.it(\"doubles\", () => expect(double(21), equals(42)))\n",
            ">)\n",
            "^ = () -> Num => < double(0) + Counter { total = 0 }.bumped() - 1 >\n"
        ),
    );

    let compile = quilon(&["compile", source.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("read the emitted IR");

    // The program's own function is emitted, and NOTHING of the harness: the blocks were
    // never checked or lowered, so nothing reaches `describe` or `it`, and reachability
    // pruning drops the module's every item with them.
    assert!(
        ir.lines()
            .any(|line| line.starts_with("define") && line.contains("@double(")),
        "the program's code must be emitted:\n{ir}"
    );
    assert_no_harness_emitted(&ir, "a release build");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The whole point, in three assertions: `out` is a run of a program whose file also holds test
/// blocks, so it exits 0 having printed [`PROGRAM_MARKER`] — which tells "the blocks were
/// erased" apart from "nothing ran at all" — and no [`ERASED_BLOCK_MARKER`].
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
        !out.stdout.contains(ERASED_BLOCK_MARKER),
        "{what} ran a test block that should have been erased:\n{}",
        out.stdout
    );
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
        // The stage lines are status; the skip itself reports nothing and builds nothing.
        assert!(
            out.stdout.is_empty() && !out.stderr.contains("error[") && !out.stderr.contains("✓"),
            "`quilon {command}` must report nothing and build nothing, said:\n{}\n{}",
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

// ── Tree shaking: an import the erased blocks were the only user of ──────────────────────

/// A suite whose harness comes in under a plain `<<`, beside code and an `^` of its own. The
/// record with a method is deliberate: its body mentions the receiver `it`, which is what once
/// kept the harness's own `it` — and everything behind it — reachable in a build.
const TESTS_BESIDE_CODE_SUITE: &str = concat!(
    "<< core.test\n",
    "double = (n :: Num) -> Num => < n * 2 >\n",
    "Counter = { total :: Num, bumped = => < it.total + 1 >}\n",
    "test.describe(\"double\", () => <\n",
    "  test.it(\"doubles\", () => expect(double(21), equals(42)))\n",
    ">)\n",
    "^ = () -> Num => < double(0) + Counter { total = 0 }.bumped() - 1 >\n"
);

/// The import needs no marker of its own. `quilon test` compiles the blocks, so their calls
/// keep the harness alive; every other command erases them, and with nothing left mentioning
/// `describe` or `it` the reachability pass shakes every item the module contributed back out.
#[test]
fn an_import_only_the_blocks_used_reaches_no_build() {
    let dir = work_dir("shaken");
    let source = write(&dir, "suite.qn", TESTS_BESIDE_CODE_SUITE);

    let tested = quilon(&["test", source.to_str().unwrap()]);
    assert_eq!(
        tested.code, 0,
        "`quilon test` compiles the blocks, so the suite runs:\n{}\n{}",
        tested.stdout, tested.stderr
    );
    assert!(
        tested.stdout.contains("1 passed, 0 failed"),
        "unexpected summary:\n{}",
        tested.stdout
    );

    let run = quilon(&["run", source.to_str().unwrap()]);
    assert_eq!(
        run.code, 0,
        "the program still runs with its blocks erased:\n{}\n{}",
        run.stdout, run.stderr
    );

    let compile = quilon(&["compile", source.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("read the emitted IR");
    assert_no_harness_emitted(&ir, "a build whose blocks were erased");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same file, counted rather than named. Naming the harness from ORDINARY code is the
/// control: every item it contributes is emitted then, which is what makes the erased build's
/// smaller count a measurement rather than an accident of the module being small.
#[test]
fn the_shaken_build_emits_only_the_programs_own_functions() {
    let dir = work_dir("shaken_count");

    let erased = write(&dir, "erased.qn", TESTS_BESIDE_CODE_SUITE);
    let compile = quilon(&["compile", erased.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let erased_ir = std::fs::read_to_string(erased.with_extension("ll")).expect("the erased IR");
    let shaken = defined_functions(&erased_ir);

    let referenced = write(
        &dir,
        "referenced.qn",
        concat!(
            "<< core.test\n",
            "double = (n :: Num) -> Num => < n * 2 >\n",
            "Counter = { total :: Num, bumped = => < it.total + 1 >}\n",
            "^ = () -> Num => <\n",
            "  test.describe(\"double\", () => <\n",
            "    test.it(\"doubles\", () => expect(double(21), equals(42)))\n",
            "  >)\n",
            "  test.reportSummary() + Counter { total = 0 }.bumped() - 1\n",
            ">\n"
        ),
    );
    let compile = quilon(&["compile", referenced.to_str().unwrap()]);
    assert_eq!(compile.code, 0, "compiling failed:\n{}", compile.stderr);
    let control_ir =
        std::fs::read_to_string(referenced.with_extension("ll")).expect("the control IR");
    let kept = defined_functions(&control_ir);

    // The control names the harness from ordinary code, so the three it calls are emitted
    // along with the lifecycle behind them: the same source with those calls inside `describe`
    // blocks emits none of it. (`failAt` and `nestingDepth` are in [`HARNESS_SYMBOLS`] because
    // no build may leak them, but nothing in the harness itself reaches them.)
    for present in [
        "describe",
        "core.test.it",
        "reportSummary",
        "enterSuite",
        "finishCase",
    ] {
        assert!(
            kept.iter().any(|line| line.contains(present)),
            "the control must emit `{present}`, or it measures nothing:\n{kept:#?}"
        );
    }
    assert!(
        shaken.len() < kept.len(),
        "erasing the blocks must shrink the build:\n         erased: {shaken:#?}\ncontrol: {kept:#?}"
    );
    for own in ["@double(", "@\"^\"(", "@main("] {
        assert!(
            shaken.iter().any(|line| line.contains(own)),
            "`{own}` is the program's own and must survive:\n{shaken:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The names `core.test` no longer exports, asserted where a user meets them: a program that
/// imports `core.http` — which needs the harness for its own cases — and defines a `green`, a
/// `red`, an `indent` and the two `report*` functions the harness now inlines.
///
/// Each definition repeats the member's former signature, since overloading is by parameter
/// types: a definition with any other signature would merely join a merged member in an
/// overload set and pass whether or not the name was still exported.
#[test]
fn an_importer_may_define_what_the_harness_no_longer_exports() {
    let dir = work_dir("shrunk_surface");
    let source = write(
        &dir,
        "own_names.qn",
        concat!(
            "<< core.io\n",
            "<< core.http\n",
            "<< core.test\n",
            "indent = (depth :: Num) -> Text => < \"..\".repeat(depth) >\n",
            "green = (text :: Text) -> Text => < \"[green]\" + text >\n",
            "red = (text :: Text) -> Text => < \"[red]\" + text >\n",
            "reportSuite = (name :: Text, depth :: Num) -> Text => < indent(depth) + name >\n",
            "reportCase = (name :: Text, depth :: Num, failed :: Bool) -> Text => <\n",
            "  indent(depth) + (failed ? red(name) : green(name))\n",
            ">\n",
            "^ = () -> Num => <\n",
            "  io.print(reportSuite(\"group\", 1))\n",
            "  io.print(reportCase(\"case\", 2, false))\n",
            "  0\n",
            ">\n"
        ),
    );

    let out = quilon(&["run", source.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "these names are the program's to define:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains("..group") && out.stdout.contains("....[green]case"),
        "the program's own definitions did not run:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The names `<< core.http` puts in a program, pinned EXACTLY. Every one arrives under its
/// module's qualified name — nothing bare but the `@` primitives — so none can collide with
/// anything the importer writes, and only the `>>`-exported ones resolve for it.
///
/// `corelib/http.qn` needs the harness for its own cases and imports it with a plain `<<`,
/// so the resolver walks into `core.test` (and `core.io` behind it), and the client's own
/// private test fixtures travel with it — carried, qualified, and unreachable from outside.
#[test]
fn importing_core_http_contributes_exactly_this_surface() {
    let tokens = Lexer::tokenize("<< core.http\n^ = () -> Num => < 0 >\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    let (linked, _sources) =
        quilon::modules::link(program, Path::new("."), None).expect("import linking failed");
    // Everything but the program's own `^` was contributed by the import.
    let mut contributed: Vec<&str> = linked
        .items
        .iter()
        .map(Item::name)
        .filter(|name| *name != "^")
        .collect();
    contributed.sort_unstable();

    let mut expected = vec![
        // core.io, behind the harness.
        "core.io.stdout",
        "core.io.stderr",
        "core.io.print",
        "core.io.eprint",
        "core.io.write",
        "@readStdin",
        // core.test: the harness, the summary, the state and the lifecycle.
        "core.test.failAt",
        "core.test.casesPassed",
        "core.test.casesFailed",
        "core.test.nestingDepth",
        "core.test.enterSuite",
        "core.test.leaveSuite",
        "core.test.caseFailing",
        "core.test.finishCase",
        "core.test.runSuite",
        "core.test.describe",
        "core.test.runCase",
        "core.test.it",
        "core.test.reportSummary",
        // core.net, and the client itself.
        "@tcpRequest",
        "core.http.Body",
        "core.http.Method",
        "core.http.Response",
        "core.http.Request",
        // The client's own PRIVATE test fixtures: carried with the module (an exported
        // item may lean on them), exported to no importer.
        "core.http.crlfReply",
        "core.http.lineFeedReply",
        "core.http.headerOr",
        "core.http.parsed",
        // core.text, merged implicitly because the client calls composable Text methods
        // (`.trim`/`.contains`) — the implementations those calls lower to, qualified so
        // they claim no name an importer could write. (The client also calls `.split` and
        // `.replaceAll`, but those are native primitives now, so they contribute nothing
        // here.)
        "core.text.trim",
        "core.text.contains",
        "core.text.replace",
        "core.text.replaceOnto",
        "core.text.replaceRemainder",
        "core.text.repeat",
        "core.text.copies",
        "core.text.double",
    ];
    expected.sort_unstable();

    assert_eq!(
        contributed, expected,
        "`<< core.http` contributes a different set of names than this pins"
    );
}

// ── Running: `quilon test` ─────────────────────────────────────────────────────────────

/// The file's own `^` is not the test run's entry point — the synthesized one is — so the
/// program's line never appears. Written with the `^` in the MIDDLE of the file, items after
/// it, since dropping it must not disturb what follows: `helper` is declared below and the
/// case that calls it still resolves.
#[test]
fn quilon_test_ignores_the_entry_point_beside_the_blocks_it_runs() {
    let dir = work_dir("beside_test");
    let source = write(
        &dir,
        "program.qn",
        &format!(
            r#"
<< core.io
<< core.test

^ = () -> $ => < io.print("{PROGRAM_MARKER}") >

helper = (n :: Num) -> Num => < n * 2 >

test.describe("helper", () => <
  test.it("doubles", () => <
    io.print("{ERASED_BLOCK_MARKER}")
    expect(helper(21), equals(42))
  >)
>)
"#
        ),
    );

    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "the case passes, so the suite must exit 0:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains(ERASED_BLOCK_MARKER) && out.stdout.contains("1 passed, 0 failed"),
        "the block beside `^` did not run:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains(PROGRAM_MARKER),
        "`quilon test` called the file's own `^`:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

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
        out.stdout.contains("4 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_case_exits_non_zero_and_the_run_carries_on() {
    let dir = work_dir("fail");
    let source = write(&dir, "suite.qn", FAILING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_ne!(out.code, 0, "a failing suite must exit non-zero");
    // Every case is reported — a failed `expect` marks its own case and nothing else — and
    // the summary tallies both ways round.
    for case in ["holds", "does not hold", "runs after the failure"] {
        assert!(
            out.stdout.contains(case),
            "`{case}` is missing from the report:\n{}",
            out.stdout
        );
    }
    assert!(
        out.stdout.contains("✓ holds") && out.stdout.contains("✗ does not hold"),
        "each case must be marked as it went:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("2 passed, 1 failed"),
        "unexpected summary:\n{}",
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
        out.stderr
            .contains("error[QN500]: assertion failed: expected 5, got 4")
            && out.stderr.contains("───"),
        "the failure must carry the coded message and an underline:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The isolation mechanism: the first failing `expect` in a case skips what is LEFT of that
/// case — the later assertions never run, so their subjects are never even evaluated — while
/// the next case starts clean.
#[test]
fn a_failed_expect_skips_the_rest_of_its_case() {
    let dir = work_dir("skip");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.test\n",
            "test.describe(\"skipping\", () => <\n",
            "  test.it(\"stops at the first failure\", () => <\n",
            "    expect(1, equals(2))\n",
            "    expect(3, equals(4))\n",
            "  >)\n",
            "  test.it(\"starts clean\", () => expect(5, equals(5)))\n",
            ">)\n"
        ),
    );
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_ne!(out.code, 0);
    assert_eq!(
        out.stderr.matches("assertion failed").count(),
        1,
        "only the first failing expect in a case may report:\n{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("expected 2, got 1") && !out.stderr.contains("expected 4, got 3"),
        "the assertions after the failure must not have run:\n{}",
        out.stderr
    );
    assert!(
        out.stdout.contains("✓ starts clean") && out.stdout.contains("1 passed, 1 failed"),
        "the next case must be unaffected:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `expect` records with the run, which only a `describe` block opens — so outside one it
/// is a compile error naming `assert` instead, rather than a program that silently drops its
/// failures. (`describe` blocks are stripped from every command but `quilon test`.)
#[test]
fn expect_outside_a_describe_block_is_a_compile_error() {
    let dir = work_dir("expect_outside");
    let source = write(
        &dir,
        "program.qn",
        "^ = () -> $ => <\n  expect(1, equals(1))\n>\n",
    );
    for command in ["check", "run", "build"] {
        let out = quilon(&[command, source.to_str().unwrap()]);
        assert_ne!(
            out.code, 0,
            "`quilon {command}` must refuse an `expect` outside a test:\n{}",
            out.stdout
        );
        assert!(
            out.stderr.contains("in a `describe` block") && out.stderr.contains("`assert`"),
            "the diagnostic must point at `assert`:\n{}",
            out.stderr
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An `expect` in a `describe` body but OUTSIDE any `it` has no case to mark: `it` is what
/// closes a case and tallies it, so such an `expect` would print a failure that no summary
/// counts — and would poison the next case, whose assertions the mark then skips. Refused at
/// compile time instead.
#[test]
fn expect_outside_an_it_case_is_a_compile_error() {
    let dir = work_dir("expect_no_case");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.test\n",
            "test.describe(\"g\", () => <\n",
            "  expect(1, equals(2))\n",
            "  test.it(\"unaffected\", () => expect(1, equals(1)))\n",
            ">)\n"
        ),
    );
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(
        out.code, 0,
        "an `expect` with no case to mark must be refused:\n{}",
        out.stdout
    );
    assert!(
        out.stderr.contains("only works inside an `it` case"),
        "the diagnostic must name the case:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── The JSON reporter, and `--only` ─────────────────────────────────────────────────────

/// Every line of `stdout` as the JSON object it is. A line that is not one fails here, so a
/// heading or a case line leaking into the stream is caught at the parse.
fn json_events(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("not a JSON event: {line:?} ({error})"))
        })
        .collect()
}

/// The `(event, path)` of every event in `events`, with a summary's path as `""`.
fn event_paths(events: &[serde_json::Value]) -> Vec<(&str, &str)> {
    events
        .iter()
        .map(|event| {
            (
                event["event"].as_str().expect("an event kind"),
                event["path"].as_str().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn the_json_reporter_emits_one_event_per_line_for_a_nested_suite() {
    let dir = work_dir("json_pass");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap(), "--reporter", "json"]);
    assert_eq!(out.code, 0, "a passing suite exits 0:\n{}", out.stderr);

    let events = json_events(&out.stdout);
    assert_eq!(
        event_paths(&events),
        vec![
            ("suite", "numbers"),
            ("case", "numbers/adds"),
            ("case", "numbers/orders"),
            ("suite", "numbers/nested"),
            ("case", "numbers/nested/still runs"),
            ("suite", "text"),
            ("case", "text/contains"),
            ("summary", ""),
        ]
    );
    assert_eq!(events[0]["depth"], 0, "an outermost suite sits at depth 0");
    assert_eq!(events[3]["depth"], 1, "a nested suite sits one deeper");
    for case in events.iter().filter(|event| event["event"] == "case") {
        assert_eq!(case["status"], "pass");
        assert!(
            case.get("message").is_none() && case.get("file").is_none(),
            "a passing case carries no failure: {case}"
        );
    }
    assert_eq!(events[7]["passed"], 4);
    assert_eq!(events[7]["failed"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_json_reporter_carries_a_failing_cases_message_file_and_line() {
    let dir = work_dir("json_fail");
    let source = write(&dir, "suite.qn", FAILING_SUITE);
    let out = quilon(&["test", source.to_str().unwrap(), "--reporter", "json"]);
    assert_ne!(out.code, 0, "a failing suite exits non-zero");

    let events = json_events(&out.stdout);
    let failed = events
        .iter()
        .find(|event| event["path"] == "arithmetic/does not hold")
        .expect("the failing case is reported");
    assert_eq!(failed["status"], "fail");
    assert_eq!(failed["message"], "assertion failed: expected 5, got 4");
    assert_eq!(failed["file"], source.to_str().unwrap());
    assert_eq!(failed["line"], 6);
    let summary = events.last().expect("a summary ends the stream");
    assert_eq!(summary["event"], "summary");
    assert_eq!(summary["passed"], 2);
    assert_eq!(summary["failed"], 1);
    // The failure's frame still goes to stderr, where a human reads it.
    assert!(
        out.stderr.contains("expected 5, got 4"),
        "the frame is missing from stderr:\n{}",
        out.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_runs_the_one_case_it_names_and_opens_nothing_else() {
    let dir = work_dir("only_case");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        source.to_str().unwrap(),
        "--reporter",
        "json",
        "--only",
        "numbers/orders",
    ]);
    assert_eq!(out.code, 0, "the selected case passes:\n{}", out.stderr);

    let events = json_events(&out.stdout);
    assert_eq!(
        event_paths(&events),
        vec![
            ("suite", "numbers"),
            ("case", "numbers/orders"),
            ("summary", "")
        ]
    );
    assert_eq!(events[2]["passed"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_with_a_suite_path_runs_every_case_under_it() {
    let dir = work_dir("only_suite");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        source.to_str().unwrap(),
        "--reporter",
        "json",
        "--only",
        "numbers",
    ]);
    assert_eq!(out.code, 0, "{}", out.stderr);

    let events = json_events(&out.stdout);
    assert_eq!(
        event_paths(&events),
        vec![
            ("suite", "numbers"),
            ("case", "numbers/adds"),
            ("case", "numbers/orders"),
            ("suite", "numbers/nested"),
            ("case", "numbers/nested/still runs"),
            ("summary", ""),
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_repeats_to_select_several_paths() {
    let dir = work_dir("only_many");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        source.to_str().unwrap(),
        "--only",
        "numbers/adds",
        "--only",
        "text/contains",
    ]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("✓ adds")
            && out.stdout.contains("✓ contains")
            && !out.stdout.contains("orders")
            && !out.stdout.contains("nested"),
        "only the two named cases run:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("2 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_with_a_path_the_suite_does_not_have_is_an_error_listing_its_paths() {
    let dir = work_dir("only_unknown");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        source.to_str().unwrap(),
        "--only",
        "numbers/adds",
        "--only",
        "numbers/divides",
    ]);
    assert_eq!(
        out.code, 1,
        "an unknown path fails the run:\n{}",
        out.stdout
    );
    assert!(
        out.stderr.contains("numbers/divides"),
        "the diagnostic names the unknown path:\n{}",
        out.stderr
    );
    for available in [
        "numbers",
        "numbers/adds",
        "numbers/nested/still runs",
        "text/contains",
    ] {
        assert!(
            out.stderr.contains(&format!("\n  {available}\n"))
                || out.stderr.ends_with(&format!("\n  {available}\n")),
            "`{available}` is missing from the listed paths:\n{}",
            out.stderr
        );
    }
    assert!(
        !out.stdout.contains("passed,"),
        "nothing ran:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── The run's state, as named functions ─────────────────────────────────────────────────

/// The run's state is a `.qn` API, not a set of runtime symbol names: a case reads how many
/// cases have passed and failed and how deep the nesting is through `core.test`'s own
/// functions.
#[test]
fn the_run_state_is_readable_through_named_functions() {
    let dir = work_dir("state");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.io\n",
            "<< core.test\n",
            "test.describe(\"outer\", () => <\n",
            "  test.it(\"sits at depth 1\", () => expect(test.nestingDepth(), equals(1)))\n",
            "  test.describe(\"inner\", () => <\n",
            "    test.it(\"sits one deeper\", () => expect(test.nestingDepth(), equals(2)))\n",
            "    test.it(\"counts the cases behind it\", () => <\n",
            "      expect(test.casesPassed(), equals(2))\n",
            "      expect(test.casesFailed(), equals(0))\n",
            "    >)\n",
            "  >)\n",
            ">)\n"
        ),
    );

    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "every case reads its own state correctly:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains("3 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The exported surface of `core.test`, asserted on the corelib itself: what a suite and the
/// synthesized entry point name, and nothing more. The report's colors and its two per-group
/// and per-case lines are written out inside `describe`, `it` and `reportSummary` rather than
/// behind exported helpers, so a name no caller outside the module has is not in the scope
/// `<< core.test` merges.
#[test]
fn the_corelib_exports_only_what_the_harness_needs() {
    let harness = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("corelib")
            .join("test.qn"),
    )
    .expect("read corelib/test.qn");

    for provided in [
        "describe",
        "it",
        "reportSummary",
        "failAt",
        "casesPassed",
        "casesFailed",
        "nestingDepth",
        "enterSuite",
        "leaveSuite",
        "caseFailing",
        "finishCase",
    ] {
        assert!(
            harness.contains(&format!(">> {provided} = ")),
            "`core.test` must export `{provided}` — a suite or the test entry point calls it"
        );
    }
    for absent in ["indent", "green", "red", "reportSuite", "reportCase"] {
        assert!(
            !harness.contains(&format!(">> {absent} = ")),
            "`core.test` must not export `{absent}` — nothing outside the module calls it, so \
             it is a name taken from every importer for nothing"
        );
    }
}

/// `assert` in a case is still FATAL: it ends the run where it failed rather than recording.
/// That is the whole difference between the two entry points.
#[test]
fn an_assert_in_a_case_is_still_fatal() {
    let dir = work_dir("fatal");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.test\n",
            "test.describe(\"fatal\", () => <\n",
            "  test.it(\"asserts\", () => assert(1, equals(2)))\n",
            "  test.it(\"never reached\", () => expect(1, equals(1)))\n",
            ">)\n"
        ),
    );
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_eq!(out.code, 101, "a failing `assert` exits 101");
    assert!(
        !out.stdout.contains("never reached") && !out.stdout.contains("passed,"),
        "a fatal assert must end the run where it failed:\n{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_runs_every_suite_it_holds() {
    let dir = work_dir("dir");
    write(&dir, "green.qn", PASSING_SUITE);
    write(&dir, "red.qn", FAILING_SUITE);
    // A program AND a suite: discovery goes by the blocks, so an `^` beside them is no reason
    // to pass the file over — this is the shape the CI step relies on finding.
    write(
        &dir,
        "mixed.qn",
        concat!(
            "<< core.test\n",
            "test.describe(\"mixed\", () => test.it(\"runs\", () => expect(true, equals(true))))\n",
            "^ = () -> Num => < 7 >\n"
        ),
    );
    // Not a suite: a program with no test blocks is passed over, not run.
    write(&dir, "program.qn", "^ = () -> Num => < 7 >\n");

    let out = quilon(&["test", dir.to_str().unwrap()]);
    assert_ne!(out.code, 0, "one suite failed, so the run failed");
    assert!(
        out.stdout.contains("green.qn")
            && out.stdout.contains("red.qn")
            && out.stdout.contains("mixed.qn"),
        "every suite should have run:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("program.qn"),
        "a file with no test blocks is not a suite:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("3 suites: 2 passed, 1 failed"),
        "unexpected per-file tally:\n{}",
        out.stdout
    );

    // Each suite's totals are its own — a process per suite is what keeps one suite's counts
    // out of another's summary.
    assert!(
        out.stdout.contains("4 passed, 0 failed") && out.stdout.contains("2 passed, 1 failed"),
        "one suite's totals leaked into another's summary:\n{}",
        out.stdout
    );
    // Off a terminal (a captured pipe, exactly what this harness gives every test), no
    // per-file compile stage ever prints — a suite's own file heading and case tree are
    // the progress `quilon test` shows.
    for stage in ["lexing", "parsing", "resolving", "checking", "generating"] {
        assert!(
            !out.stderr.lines().any(|line| line.starts_with(stage)),
            "a per-stage line leaked into a multi-suite run: {}",
            out.stderr
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_with_no_suites_succeeds_and_says_so() {
    let dir = work_dir("empty");
    write(&dir, "program.qn", "^ = () -> Num => < 0 >\n");
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
        "<< core.test\ndescribe(\"g\", () => expect(1, equals(\"one\")))\n",
    );
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0, "a suite that fails to type-check must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A suite that imports no harness at all is told so at its own `test.describe` — rather
/// than at the entry point the compiler synthesized, which has no location — with the
/// import that fixes it named.
#[test]
fn a_suite_without_a_harness_is_reported_at_its_own_describe() {
    let dir = work_dir("noimport");
    let source = write(&dir, "suite.qn", "\ntest.describe(\"g\", () => 0)\n");
    let out = quilon(&["test", source.to_str().unwrap()]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("suite.qn:2:") && out.stderr.contains("not an imported module"),
        "the diagnostic must point at the `test.describe` call:\n{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("<< core.test"),
        "the diagnostic must name the import that fixes it:\n{}",
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

/// The other shape tests come in: a module — `>>` exports, whatever fixtures its cases need,
/// and no `^`. Erasing its blocks leaves nothing to run, so `build` passes over it, while
/// `quilon test` runs the same file as a suite.
#[test]
fn a_module_with_exports_and_tests_but_no_entry_point_is_not_a_program() {
    let dir = work_dir("helpers");
    let source = write(
        &dir,
        "suite.qn",
        concat!(
            "<< core.test\n",
            ">> double = (n :: Num) -> Num => < n * 2 >\n",
            "test.describe(\"double\", () => <\n",
            "  test.it(\"doubles\", () => expect(double(21), equals(42)))\n",
            ">)\n"
        ),
    );

    let build = quilon(&["build", source.to_str().unwrap()]);
    assert_eq!(
        build.code, 0,
        "`quilon build` must pass over a module that is not a program:\n{}\n{}",
        build.stdout, build.stderr
    );
    assert!(
        build.stdout.is_empty() && !build.stderr.contains("error[") && !build.stderr.contains("✓"),
        "the skip must report nothing and build nothing, said:\n{}\n{}",
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
        out.stdout.contains("passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
}

/// A corelib module's own suite sits beside the code it tests, and the import resolver drops an
/// imported module's blocks — so `corelib/http.qn` runs only when it is the file named. Gated
/// here as well as in CI: the blocks are compiled by nothing else, so a type error in one is
/// invisible to every other command.
#[test]
fn the_corelib_http_suite_passes_when_the_module_is_the_file_named() {
    let module = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corelib")
        .join("http.qn");
    let out = quilon(&["test", module.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "corelib/http.qn must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    for group in [
        "the status line",
        "headers",
        "the body",
        "line endings",
        "the Method sum",
        "reading a URL apart",
        "serialising a request",
        "a round trip",
    ] {
        assert!(
            out.stdout.contains(group),
            "the `{group}` group is missing from the report:\n{}",
            out.stdout
        );
    }
    assert!(
        out.stdout.contains("71 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
}

/// The same gate for `corelib/text.qn` — the self-hosted Text methods' own suite, the one
/// compilation of those blocks.
#[test]
fn the_corelib_text_suite_passes_when_the_module_is_the_file_named() {
    let module = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corelib")
        .join("text.qn");
    let out = quilon(&["test", module.to_str().unwrap()]);
    assert_eq!(
        out.code, 0,
        "corelib/text.qn must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    for group in [
        "split",
        "trim",
        "contains",
        "replace",
        "repeat",
        "grapheme access",
    ] {
        assert!(
            out.stdout.contains(group),
            "the `{group}` group is missing from the report:\n{}",
            out.stdout
        );
    }
    assert!(
        out.stdout.contains("20 passed, 0 failed"),
        "unexpected summary:\n{}",
        out.stdout
    );
}

// ── The shipped example: tests beside the code, and what each command does with them ─────

/// The shipped demonstration, built and run: `examples/tests_alongside_code.qn` holds its `>>`
/// exports, its `^`, and the `describe` blocks checking them in ONE file, so a build of it
/// prints the program's own line and never a word of test output — under the JIT and a native
/// build alike.
#[test]
fn the_shipped_example_builds_without_the_tests_beside_its_entry_point() {
    let source = example("tests_alongside_code.qn");

    let run = quilon(&["run", source.to_str().unwrap()]);
    assert_ran_without_its_tests("`quilon run` on the example", &run);

    match available_linker() {
        Some(linker) => {
            let dir = work_dir("shipped");
            let native = build_and_execute(&source, &dir, linker);
            assert_ran_without_its_tests("the built example", &native);
            let _ = std::fs::remove_dir_all(&dir);
        }
        None => eprintln!("skipping the native half: need a linker (`clang` or `gcc`) on PATH"),
    }
}

/// The other direction, on the very file the build erased: `quilon test` compiles those blocks,
/// so the line no build ever showed is on stdout and its case is reported — while the file's
/// own `^` stays uncalled.
#[test]
fn the_shipped_example_runs_its_tests_under_quilon_test() {
    let source = example("tests_alongside_code.qn");
    let out = quilon(&["test", source.to_str().unwrap()]);

    assert_eq!(
        out.code, 0,
        "the shipped suite must pass:\n{}\n{}",
        out.stdout, out.stderr
    );
    assert!(
        out.stdout.contains(ERASED_BLOCK_MARKER),
        "the test block did not run under `quilon test`:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains(PROGRAM_MARKER),
        "`quilon test` called the example's own `^`:\n{}",
        out.stdout
    );
    for expected in [
        "slugify",
        "wordCount",
        "the erased block",
        "4 passed, 0 failed",
    ] {
        assert!(
            out.stdout.contains(expected),
            "`{expected}` is missing from the report:\n{}",
            out.stdout
        );
    }
}

// ── `--binary`: a native, debuggable test executable ─────────────────────────────────────

/// `quilon test <file> --binary <out>`, requiring a linker on PATH (`--binary` always links
/// with `clang`) — the produced executable is native, so there is nothing to assert
/// without one.
fn build_binary(dir: &Path, source: &Path, name: &str, extra: &[&str]) -> Option<PathBuf> {
    if !tool_available("clang") {
        return None;
    }
    ensure_runtime_lib(
        Path::new(env!("CARGO_BIN_EXE_quilon"))
            .parent()
            .expect("the compiler's directory"),
    );
    let binary = dir.join(name);
    let mut arguments = vec![
        "test",
        source.to_str().unwrap(),
        "--binary",
        binary.to_str().unwrap(),
    ];
    arguments.extend_from_slice(extra);
    let build = quilon(&arguments);
    assert_eq!(
        build.code, 0,
        "`quilon test --binary` failed:\n{}",
        build.stderr
    );
    Some(binary)
}

#[test]
fn binary_builds_an_executable_that_passes_a_passing_suite() {
    let dir = work_dir("binary_pass");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    match build_binary(&dir, &source, "suite_binary", &[]) {
        Some(binary) => {
            let run = execute(&binary);
            assert_eq!(
                run.code, 0,
                "a passing suite's binary must exit 0:\n{}\n{}",
                run.stdout, run.stderr
            );
            assert!(
                run.stdout.contains("4 passed, 0 failed"),
                "unexpected summary:\n{}",
                run.stdout
            );
        }
        None => eprintln!("skipping the native half: need `clang` on PATH"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn binary_of_a_failing_suite_exits_non_zero() {
    let dir = work_dir("binary_fail");
    let source = write(&dir, "suite.qn", FAILING_SUITE);
    match build_binary(&dir, &source, "suite_binary", &[]) {
        Some(binary) => {
            let run = execute(&binary);
            assert_ne!(run.code, 0, "a failing suite's binary must exit non-zero");
            assert!(
                run.stdout.contains("2 passed, 1 failed"),
                "unexpected summary:\n{}",
                run.stdout
            );
        }
        None => eprintln!("skipping the native half: need `clang` on PATH"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--only`, given at the build, is baked into the binary: running it plain reproduces the
/// filtered run, with no argument or environment variable needed at launch — the shape a
/// debugger's launch configuration wants.
#[test]
fn binary_honours_only_and_runs_nothing_else() {
    let dir = work_dir("binary_only");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    match build_binary(&dir, &source, "suite_binary", &["--only", "numbers/orders"]) {
        Some(binary) => {
            let run = execute(&binary);
            assert_eq!(
                run.code, 0,
                "the selected case passes:\n{}\n{}",
                run.stdout, run.stderr
            );
            assert!(
                run.stdout.contains("✓ orders") && run.stdout.contains("1 passed, 0 failed"),
                "only the selected case must have run:\n{}",
                run.stdout
            );
            for excluded in ["adds", "nested", "text", "contains"] {
                assert!(
                    !run.stdout.contains(excluded),
                    "`{excluded}` must not appear — it was excluded by `--only`:\n{}",
                    run.stdout
                );
            }
        }
        None => eprintln!("skipping the native half: need `clang` on PATH"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn binary_with_an_unknown_only_path_fails_the_build_listing_the_suites_paths() {
    let dir = work_dir("binary_only_unknown");
    let source = write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        source.to_str().unwrap(),
        "--binary",
        dir.join("unused").to_str().unwrap(),
        "--only",
        "numbers/divides",
    ]);
    assert_ne!(out.code, 0, "an unknown `--only` path must fail the build");
    assert!(
        out.stderr.contains("numbers/divides") && out.stderr.contains("numbers/adds"),
        "the diagnostic must name the unknown path and list what the suite has:\n{}",
        out.stderr
    );
    assert!(
        !dir.join("unused").exists(),
        "nothing should have been built"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn binary_targeting_a_directory_is_a_diagnostic_error() {
    let dir = work_dir("binary_dir");
    write(&dir, "suite.qn", PASSING_SUITE);
    let out = quilon(&[
        "test",
        dir.to_str().unwrap(),
        "--binary",
        dir.join("out").to_str().unwrap(),
    ]);
    assert_ne!(out.code, 0, "`--binary` on a directory must fail");
    assert!(
        out.stderr.contains("--binary") && out.stderr.contains(&dir.display().to_string()),
        "the diagnostic must name the flag and the directory given:\n{}",
        out.stderr
    );
    assert!(!dir.join("out").exists(), "nothing should have been built");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The default path is `.`, a directory — the same diagnostic as naming one explicitly.
#[test]
fn binary_with_the_default_path_is_the_same_diagnostic_error() {
    let dir = work_dir("binary_default_path");
    write(&dir, "suite.qn", PASSING_SUITE);
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["test", "--binary", dir.join("out").to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .expect("spawn quilon test");
    assert_ne!(
        out.status.code().unwrap_or(-1),
        0,
        "`--binary` with the default `.` path must fail"
    );
    assert!(!dir.join("out").exists(), "nothing should have been built");
    let _ = std::fs::remove_dir_all(&dir);
}
