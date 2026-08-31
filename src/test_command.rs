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

use crate::driver::{self, TestBlocks};
use crate::jit;
use crate::source_extension;

/// Directory names never searched for suites: walking build output is slow and finds
/// nothing. (Hidden entries are skipped separately, by their leading dot.)
const SKIPPED_DIRECTORIES: &[&str] = &["target", "node_modules"];

/// Run the suites at `root` — one file, or every suite under a directory — and return how
/// many FAILED. A suite fails when a case fails, or when the file does not compile.
///
/// A single suite runs here, in this process, under its own path as a heading. Several run
/// one process each (see the module docs), and a closing line tallies them.
pub fn run(root: &Path) -> usize {
    // A path that is not there is a failure, not an empty run: a mistyped path in a CI
    // invocation must not report success.
    if !root.exists() {
        eprintln!("❌ no such file or directory: {}", root.display());
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
            usize::from(!compile_and_run(suite))
        }
        many => {
            let failed = many
                .iter()
                .filter(|suite| !run_in_its_own_process(suite))
                .count();
            println!(
                "{} suites: {} passed, {failed} failed",
                many.len(),
                many.len() - failed
            );
            failed
        }
    }
}

/// Run one suite by re-invoking this binary on it, its output going straight to our own
/// stdout/stderr. `true` when the child exited 0.
///
/// Handing the suite to a child is what keeps a fatal `assert` — which exits — from ending
/// the whole run at the first failing suite, and keeps each suite's tally its own. Named with exactly one file, the
/// child takes the single-suite path above and does not spawn again.
fn run_in_its_own_process(suite: &Path) -> bool {
    let quilon = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            // Without our own path there is no child to run in. Fall back to this process,
            // where a failing case ends the run early — and say so rather than looking fine.
            eprintln!(
                "❌ cannot locate the quilon binary ({error}); running {} here, so a failing \
                 case will end the run",
                suite.display()
            );
            println!("{}", suite.display());
            return compile_and_run(suite);
        }
    };
    match Command::new(quilon).arg("test").arg(suite).status() {
        Ok(status) => status.success(),
        Err(error) => {
            eprintln!("❌ could not run {}: {error}", suite.display());
            false
        }
    }
}

/// Type-check `suite` with its test blocks compiled in, then JIT the synthesized entry
/// point. `true` when the run exited 0. A front-end failure is reported like any other
/// compile error and counts as a failure.
fn compile_and_run(suite: &Path) -> bool {
    let checked = match driver::front_end_with(suite, TestBlocks::Run) {
        Ok(checked) => checked,
        Err(error) => {
            eprintln!("{}", error);
            return false;
        }
    };

    // A suite takes no arguments of its own — the entry point the front end synthesizes has
    // no parameters — so `argv` is just the program's path, the way a native build sees it.
    let argv = [suite.to_string_lossy().into_owned()];

    match jit::run_program(
        &checked.program,
        checked.types,
        checked.defer,
        checked.sources,
        &argv,
    ) {
        Ok(code) => code == 0,
        Err(error) => {
            eprintln!("❌ Runtime error in {}: {}", suite.display(), error);
            false
        }
    }
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
