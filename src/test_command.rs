//! `quilon test` — find a project's test suites and run them.
//!
//! A suite is a `.qn` file with top-level `describe(…)` blocks (see
//! [`crate::ast::TEST_BLOCK_MARKER`]); the front end synthesizes the entry point that runs
//! those blocks. The blocks may sit beside the code they test — the file's own `^` included,
//! and that `^` is not the test run's, so it is dropped rather than called. Running is always
//! through the in-process JIT, never a native build.
//!
//! ONE suite per process. The registry a suite records through is per thread, so its counts
//! and its summary have to be its own; and a case may use the fatal `assert`, which exits
//! 101 there and then and would take every later suite with it. A run of many therefore
//! spawns this same binary once per suite, stdio inherited so each report goes straight
//! through.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diagnostic::{Code, Diagnostic};
use crate::driver::{self, TestBlocks};
use crate::jit;
use crate::quips;
use crate::source_extension;
use crate::source_map::SourceMap;
use crate::status::{Stage, Status, format_duration};

/// Directory names never searched for suites: walking build output is slow and finds
/// nothing. (Hidden entries are skipped separately, by their leading dot.)
const SKIPPED_DIRECTORIES: &[&str] = &["target", "node_modules"];

/// Run the suites at `root` — one file, or every suite under a directory — and return how
/// many FAILED. A suite fails when a case fails, or when the file does not compile.
///
/// A single suite runs here, in this process, under its own path as a heading. Several run
/// one process each (see the module docs), and a closing line tallies them. `quiet` keeps
/// the runner's own status lines off stderr; the suites' reports print regardless.
pub fn run(root: &Path, quiet: bool) -> usize {
    let status = Status::for_command(quiet);
    // A path that is not there is a failure, not an empty run: a mistyped path in a CI
    // invocation must not report success.
    if !root.exists() {
        report(
            Code::SourceNotReadable,
            format!("no such file or directory: {}", root.display()),
            &status,
        );
        return 1;
    }

    let suites = discover(root);
    match suites.as_slice() {
        [] => {
            println!(
                "no tests found in {} — a test file imports `core.test` and has top-level \
                 `test.{}` blocks",
                root.display(),
                crate::ast::display_name(crate::ast::TEST_BLOCK_MARKER)
            );
            0
        }
        [suite] => {
            println!("{}", suite.display());
            usize::from(!compile_and_run(suite, &status))
        }
        many => {
            let failed = many
                .iter()
                .filter(|suite| !run_in_its_own_process(suite, quiet))
                .count();
            let quip = match failed {
                0 => quips::pick(quips::TESTS_PASSED),
                _ => quips::pick(quips::TESTS_FAILED),
            };
            println!(
                "{} suites: {} passed, {failed} failed. {quip}",
                many.len(),
                many.len() - failed
            );
            failed
        }
    }
}

/// Print a diagnostic with no source location, the way every report is printed.
fn report(code: Code, message: String, status: &Status) {
    eprintln!(
        "{}",
        Diagnostic::new(code, message).render(&SourceMap::default(), status.color())
    );
}

/// Run one suite by re-invoking this binary on it, its output going straight to our own
/// stdout/stderr. `true` when the child exited 0.
///
/// Handing the suite to a child is what keeps a fatal `assert` — which exits — from ending
/// the whole run at the first failing suite, and keeps each suite's tally its own. Named with exactly one file, the
/// child takes the single-suite path above and does not spawn again.
fn run_in_its_own_process(suite: &Path, quiet: bool) -> bool {
    let status = Status::for_command(quiet);
    let quilon = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            // Without our own path there is no child to run in. Fall back to this process,
            // where a failing case ends the run early — and say so rather than looking fine.
            report(
                Code::SourceNotReadable,
                format!(
                    "cannot locate the quilon binary ({error}); running {} here, so a \
                     failing case will end the run",
                    suite.display()
                ),
                &status,
            );
            println!("{}", suite.display());
            return compile_and_run(suite, &status);
        }
    };
    let mut command = Command::new(quilon);
    command.arg("test").arg(suite);
    if quiet {
        command.arg("--quiet");
    }
    match command.status() {
        Ok(exit) => exit.success(),
        Err(error) => {
            report(
                Code::SourceNotReadable,
                format!("could not run {}: {error}", suite.display()),
                &status,
            );
            false
        }
    }
}

/// Type-check `suite` with its test blocks compiled in, then JIT the synthesized entry
/// point. `true` when the run exited 0. A front-end failure is reported like any other
/// compile error and counts as a failure. The suite's own report is `core.test`'s; the
/// line after it — the verdict, the elapsed time, and a quip — is the runner's.
fn compile_and_run(suite: &Path, status: &Status) -> bool {
    let checked = match driver::front_end_reporting(suite, TestBlocks::Run, status) {
        Ok(checked) => checked,
        Err(error) => {
            eprintln!("{}", error.render(status.color()));
            return false;
        }
    };

    // A suite takes no arguments of its own — the entry point the front end synthesizes has
    // no parameters — so `argv` is just the program's path, the way a native build sees it.
    let argv = [suite.to_string_lossy().into_owned()];

    status.stage(Stage::Generating);
    status.clear();
    let passed = match jit::run_program(
        &checked.program,
        checked.types,
        checked.defer,
        checked.sources,
        &argv,
    ) {
        Ok(code) => code == 0,
        Err(error) => {
            report(
                Code::CodegenFailed,
                format!("in {}: {error}", suite.display()),
                status,
            );
            false
        }
    };
    let (mark, quip) = match passed {
        true => (status.paint("32", "✓"), quips::pick(quips::TESTS_PASSED)),
        false => (status.paint("31", "✗"), quips::pick(quips::TESTS_FAILED)),
    };
    status.done_with(&format!(
        "{mark} {} ({}) — {quip}",
        suite.display(),
        status.paint("2", &format_duration(status.elapsed()))
    ));
    passed
}

/// Every suite at `root`, sorted so a run's order is the same everywhere: `root` itself when
/// it is a suite file, or each suite found under it when it is a directory. See
/// [`driver::is_test_suite`] for what qualifies.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut suites = Vec::new();
    match root.is_dir() {
        true => collect(root, &mut suites),
        false => consider(root, &mut suites),
    }
    suites.sort();
    suites
}

fn consider(file: &Path, suites: &mut Vec<PathBuf>) {
    if driver::is_test_suite(file) {
        suites.push(file.to_path_buf());
    }
}

/// Walk `directory`, collecting the suites in it and below. Symlinks are never followed —
/// `file_type` reports the entry itself, not its target — so a link back up the tree cannot
/// send this into unbounded recursion, and a linked directory's suites are not run twice.
fn collect(directory: &Path, suites: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name.as_ref()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let child = entry.path();
        if kind.is_dir() {
            collect(&child, suites);
        } else if kind.is_file() && name.ends_with(source_extension::EXTENSION) {
            consider(&child, suites);
        }
    }
}
