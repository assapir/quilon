//! `quilon test` — find a project's test suites and run them.
//!
//! A suite is a `.qn` file with top-level `describe(…)` blocks (see
//! [`crate::ast::TEST_BLOCK_MARKER`]) and no `^` of its own. The front end synthesizes the
//! entry point that runs those blocks in order and ends with the reporter's summary, whose
//! result is the file's exit status; running is always through the in-process JIT, never a
//! native build.
//!
//! Each file runs on its OWN thread. The registry the harness records into is per-thread
//! (see `quilon_rt::test_registry`), so a thread per file is what keeps one suite's totals
//! out of the next one's summary — with no state to reset between runs.

use std::path::{Path, PathBuf};

use crate::driver::{self, TestBlocks};
use crate::jit;

/// Directory entries never searched for suites: version control and build output, plus
/// anything hidden. Walking them is slow and finds nothing.
const SKIPPED_DIRECTORIES: &[&str] = &["target", "node_modules"];

/// What a run of one file, or of a whole tree, came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Suites that ran and reported every case passing.
    pub passed: usize,
    /// Suites that failed — a case reported, or the file did not compile.
    pub failed: usize,
}

impl Outcome {
    /// The process exit status: 0 only when nothing failed.
    pub fn exit_code(&self) -> i32 {
        match self.failed {
            0 => 0,
            _ => 1,
        }
    }
}

/// Run the suites at `root` — one file, or every suite under a directory. `arguments` is
/// forwarded to each program as its `^` arguments would be, after the file's own path.
///
/// Prints each file's path before running it (the suite's own reporter prints the rest),
/// and a closing line naming how many files failed when more than one ran.
pub fn run(root: &Path, arguments: &[String]) -> Outcome {
    let suites = discover(root);
    if suites.is_empty() {
        println!(
            "no tests found in {} — a test file has top-level `{}` blocks",
            root.display(),
            crate::ast::TEST_BLOCK_MARKER
        );
        return Outcome::default();
    }

    let mut outcome = Outcome::default();
    for suite in &suites {
        println!("{}", suite.display());
        match run_one(suite, arguments) {
            true => outcome.passed += 1,
            false => outcome.failed += 1,
        }
    }

    if suites.len() > 1 {
        println!(
            "{} suites: {} passed, {} failed",
            suites.len(),
            outcome.passed,
            outcome.failed
        );
    }
    outcome
}

/// Run one suite on a thread of its own — which is what isolates its totals from the next
/// suite's, the registry being per-thread. `true` when every case passed.
fn run_one(suite: &Path, arguments: &[String]) -> bool {
    // Everything the run needs is built INSIDE the thread: a checked program holds `Rc`s
    // and so cannot cross a thread boundary, while `&Path`/`&[String]` can.
    let outcome =
        std::thread::scope(|scope| scope.spawn(|| compile_and_run(suite, arguments)).join());
    match outcome {
        Ok(passed) => passed,
        Err(_) => {
            eprintln!("❌ {} panicked while running", suite.display());
            false
        }
    }
}

/// Type-check `suite` with its test blocks compiled in, then JIT the synthesized entry
/// point. `true` when the run exited 0. A front-end failure is reported like any other
/// compile error and counts as a failure.
fn compile_and_run(suite: &Path, arguments: &[String]) -> bool {
    let checked = match driver::front_end_with(suite, TestBlocks::Run) {
        Ok(checked) => checked,
        Err(error) => {
            eprintln!("{}", error);
            return false;
        }
    };

    // The argument vector a native build would see: the program's own path, then the
    // caller's arguments — the same shape `quilon run` builds.
    let mut argv = Vec::with_capacity(arguments.len() + 1);
    argv.push(suite.to_string_lossy().into_owned());
    argv.extend(arguments.iter().cloned());

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

/// Every suite at `root`, sorted so a run's order is the same everywhere: `root` itself
/// when it is a suite file, or each suite found under it when it is a directory.
///
/// A file qualifies by having top-level test blocks, which takes a parse — so a `.qn` file
/// that does not parse is passed over rather than reported here. `quilon check` is what
/// reports on a file that cannot be read.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut suites = Vec::new();
    collect(root, &mut suites);
    suites.sort();
    suites
}

fn collect(path: &Path, suites: &mut Vec<PathBuf>) {
    if path.is_file() {
        if driver::has_test_blocks(path).unwrap_or(false) {
            suites.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name.as_ref()) {
            continue;
        }
        // Descend into a directory; consider a file only when it is a Quilon source.
        if child.is_dir() || child.extension().is_some_and(|extension| extension == "qn") {
            collect(&child, suites);
        }
    }
}
