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

pub use quilon_rt::test_registry::Reporter;
use quilon_rt::test_registry::{selects, set_reporter, set_selection};

use crate::ast::{self, Expression, Statement, TEST_BLOCK_MARKER, TEST_CASE_MARKER};
use crate::build::{self, DebugSource};
use crate::diagnostic::{Code, Diagnostic};
use crate::driver::{self, TestBlocks};
use crate::jit;
use crate::quips;
use crate::source_extension;
use crate::source_map::SourceMap;
use crate::status::{Status, format_duration};

/// Directory names never searched for suites: walking build output is slow and finds
/// nothing. (Hidden entries are skipped separately, by their leading dot.)
const SKIPPED_DIRECTORIES: &[&str] = &["target", "node_modules"];

/// How a run was asked to report and what it was asked to run.
pub struct Options {
    pub reporter: Reporter,
    /// The `/`-joined suite and case paths selected; empty selects everything.
    pub only: Vec<String>,
    /// Silence the runner's own status lines (stage progress, the per-suite verdict and
    /// tally). Diagnostics still print. Always on under the JSON reporter, which stays pure
    /// NDJSON on stdout with no status noise alongside it.
    pub quiet: bool,
}

/// The status this run reports through: silent under `--quiet`, and always under the JSON
/// reporter, which carries no quips or progress lines of its own.
fn status_for(options: &Options) -> Status {
    Status::for_command(options.quiet || options.reporter == Reporter::Json)
}

/// Run the suites at `root` — one file, or every suite under a directory — and return how
/// many FAILED. A suite fails when a case fails, when the file does not compile, or when a
/// selected path is not in it.
///
/// A single suite runs here, in this process, under its own path as a heading. Several run
/// one process each (see the module docs), and a closing line tallies them. The JSON
/// reporter writes the suites' events and nothing else.
pub fn run(root: &Path, options: &Options) -> usize {
    let status = status_for(options);
    // A path that is not there is a failure, not an empty run: a mistyped path in a CI
    // invocation must not report success.
    if !require_existing(root, &status) {
        return 1;
    }
    if !options.only.is_empty() && root.is_dir() {
        status.clear();
        eprintln!(
            "`--only` selects within one suite file; give the file rather than {}",
            root.display()
        );
        return 1;
    }

    let human = options.reporter == Reporter::Human;
    let suites = discover(root);
    match suites.as_slice() {
        [] => {
            println!(
                "no tests found in {} — a test file imports `core.test` and has top-level \
                 `test.{}` blocks",
                root.display(),
                ast::display_name(TEST_BLOCK_MARKER)
            );
            0
        }
        [suite] => {
            if human {
                println!("{}", suite.display());
            }
            usize::from(!compile_and_run(suite, options, &status))
        }
        many => {
            let failed = many
                .iter()
                .filter(|suite| !run_in_its_own_process(suite, options))
                .count();
            if human {
                let quip = match failed {
                    0 => format!(". {}", quips::pick(quips::TESTS_PASSED)),
                    _ => format!(". {}", quips::pick(quips::TESTS_FAILED)),
                };
                println!(
                    "{} suites: {} passed, {failed} failed{quip}",
                    many.len(),
                    many.len() - failed
                );
            }
            failed
        }
    }
}

/// Print a diagnostic with no source location, the way every report is printed.
fn report(code: Code, message: String, status: &Status) {
    status.report(&Diagnostic::new(code, message), &SourceMap::default());
}

/// Whether `root` exists — reporting "no such file or directory" and returning `false` if
/// not. A mistyped path must fail rather than silently finding nothing.
fn require_existing(root: &Path, status: &Status) -> bool {
    if root.exists() {
        return true;
    }
    report(
        Code::SourceNotReadable,
        format!("no such file or directory: {}", root.display()),
        status,
    );
    false
}

/// Whether `only` names a path `program` does not have. When it does, the mismatch and the
/// suite's actual paths are reported on stderr. An empty `only` selects everything and is
/// never rejected.
fn reject_unknown_selection(
    program: &ast::Program,
    only: &[String],
    suite: &Path,
    status: &Status,
) -> bool {
    if only.is_empty() {
        return false;
    }
    let available = available_paths(program);
    let Some(unknown) = only
        .iter()
        .find(|selected| !available.iter().any(|path| selects(selected, path)))
    else {
        return false;
    };
    status.clear();
    eprintln!(
        "no suite or case `{unknown}` in {}; its paths are:\n  {}",
        suite.display(),
        available.join("\n  ")
    );
    true
}

/// Run one suite by re-invoking this binary on it, its output going straight to our own
/// stdout/stderr. `true` when the child exited 0.
///
/// Handing the suite to a child is what keeps a fatal `assert` — which exits — from ending
/// the whole run at the first failing suite, and keeps each suite's tally its own. Named with exactly one file, the
/// child takes the single-suite path above and does not spawn again.
fn run_in_its_own_process(suite: &Path, options: &Options) -> bool {
    let status = status_for(options);
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
            if options.reporter == Reporter::Human {
                println!("{}", suite.display());
            }
            return compile_and_run(suite, options, &status);
        }
    };
    let reporter = match options.reporter {
        Reporter::Human => "human",
        Reporter::Json => "json",
    };
    let mut command = Command::new(quilon);
    command
        .arg("test")
        .arg(suite)
        .arg("--reporter")
        .arg(reporter);
    if options.quiet {
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
/// point under `options`. `true` when the run exited 0. A front-end failure is reported
/// like any other compile error and counts as a failure, as does a selected path the suite
/// does not have. The suite's own report is `core.test`'s; the line after it — the verdict,
/// the elapsed time, and a quip — is the runner's, printed only for the human reporter.
///
/// Compiling never shows the per-stage progress `check`/`build`/`compile` do — a suite's own
/// case tree is the progress that matters here. `status.compiling` is the one transient line
/// (live-terminal only), and the front end runs under its own silent status.
fn compile_and_run(suite: &Path, options: &Options, status: &Status) -> bool {
    status.compiling(&suite.display().to_string());
    let checked = match driver::front_end_with(suite, TestBlocks::Run) {
        Ok(checked) => checked,
        Err(error) => {
            status.report(&error.diagnostic, &error.sources);
            return false;
        }
    };

    if reject_unknown_selection(&checked.program, &options.only, suite, status) {
        return false;
    }
    set_reporter(options.reporter);
    set_selection(options.only.clone());

    // A suite takes no arguments of its own — the entry point the front end synthesizes has
    // no parameters — so `argv` is just the program's path, the way a native build sees it.
    let argv = [suite.to_string_lossy().into_owned()];

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

/// Build the single suite at `root` into a native, debuggable executable at `out` instead of
/// running it — always with DWARF debug info (the `quilon build --debug` path), so a
/// debugger can step through a case. `true` on a successful build.
///
/// `root` must be one suite file: a directory (the default `.` included) cannot name the
/// one entry point a build produces. `only`, when given, is checked against the suite's
/// paths exactly as [`compile_and_run`] checks it, then baked in by pruning every
/// `describe`/`it` it excludes from the program before code generation — the produced
/// binary carries no code for what was excluded and needs no selection at run time to
/// honour it, so running it plain reproduces the filtered run.
pub fn build_binary(root: &Path, only: &[String], out: &Path, status: &Status) -> bool {
    if !require_existing(root, status) {
        return false;
    }
    if root.is_dir() {
        status.clear();
        eprintln!(
            "`--binary` builds one suite file; give the file rather than {}",
            root.display()
        );
        return false;
    }
    if discover(root).is_empty() {
        report(
            Code::SourceNotReadable,
            format!(
                "no tests found in {} — a test file imports `core.test` and has top-level \
                 `test.{}` blocks",
                root.display(),
                ast::display_name(TEST_BLOCK_MARKER)
            ),
            status,
        );
        return false;
    }

    status.compiling(&root.display().to_string());
    let mut checked = match driver::front_end_with(root, TestBlocks::Run) {
        Ok(checked) => checked,
        Err(error) => {
            status.report(&error.diagnostic, &error.sources);
            return false;
        }
    };

    if reject_unknown_selection(&checked.program, only, root, status) {
        return false;
    }
    if !only.is_empty() {
        prune_unselected(&mut checked.program, only);
    }

    let debug_source = DebugSource { file: root };
    match build::build_native(
        &checked.program,
        checked.types,
        checked.defer,
        checked.sources,
        out,
        "clang",
        Some(&debug_source),
        status,
    ) {
        Ok(()) => {
            status.done(
                &format!("{} → {}", root.display(), out.display()),
                quips::pick(quips::SUCCESS),
            );
            true
        }
        Err(error) => {
            report(Code::BuildFailed, error, status);
            false
        }
    }
}

/// Whether some path in `only` names `path` itself or an ancestor suite of it — the same
/// test the runtime applies in `__test_case_selected`/`__test_suite_selected`
/// (`quilon-rt::test_registry`), replicated here so a `--binary` build can drop what a
/// selection excludes at compile time instead of asking the runtime to skip it.
fn covered(path: &str, only: &[String]) -> bool {
    only.iter().any(|selected| selects(selected, path))
}

/// Whether some path in `only` lies under the suite at `path` — the suite has to stay (and
/// be entered) to reach it, even though `path` itself was not named.
fn nests_a_selection(path: &str, only: &[String]) -> bool {
    only.iter().any(|selected| selects(path, selected))
}

/// Drop every `describe`/`it` in the synthesized entry's body that `only` excludes, pruning
/// the body of every `describe` that survives. `only` must not be empty — an empty
/// selection runs everything, so there is nothing to prune.
fn prune_unselected(program: &mut ast::Program, only: &[String]) {
    for item in &mut program.items {
        if let ast::Item::FunctionDeclaration(entry) = item
            && entry.name == "^"
        {
            prune_block(&mut entry.body, "", only);
        }
    }
}

/// Prune every `describe`/`it` statement directly inside `expression` — a synthesized
/// entry's top-level `Block`, or a `describe`'s `Lambda` body.
fn prune_block(expression: &mut Expression, prefix: &str, only: &[String]) {
    match expression {
        Expression::Block { statements, .. } => {
            statements.retain_mut(|statement| match statement {
                Statement::Expression(expression) => prune_call(expression, prefix, only),
                _ => true,
            });
        }
        Expression::Lambda { body, .. } => prune_block(body, prefix, only),
        _ => {}
    }
}

/// Prune one statement of a `describe`/`it` body: `false` drops it — a case `only` excludes,
/// or a suite carrying no selected case — and `true` keeps it, pruning a kept suite's own
/// body in turn. Anything besides a `describe`/`it` call (the summary call that closes the
/// synthesized entry) is kept untouched.
fn prune_call(expression: &mut Expression, prefix: &str, only: &[String]) -> bool {
    let Expression::Call {
        function,
        arguments,
        ..
    } = expression
    else {
        return true;
    };
    let Expression::Identifier { name, .. } = function.as_ref() else {
        return true;
    };
    if name != TEST_BLOCK_MARKER && name != TEST_CASE_MARKER {
        return true;
    }
    let [Expression::String { value, .. }, body] = arguments.as_mut_slice() else {
        return true;
    };
    let path = match prefix.is_empty() {
        true => value.clone(),
        false => format!("{prefix}/{value}"),
    };
    if name == TEST_CASE_MARKER {
        return covered(&path, only);
    }
    let keep = covered(&path, only) || nests_a_selection(&path, only);
    if keep {
        prune_block(body, &path, only);
    }
    keep
}

/// Every suite and case path in `program`, in source order — each `describe`/`it` call
/// with a literal name, the names joined by `/` through the nesting. Read off the entry
/// point the front end synthesized, which holds the file's blocks.
///
/// ponytail: a static read, so a case whose name is computed (an `it` inside `.each`) is
/// listed under no path; make the runtime record the paths it is asked about if that is ever
/// wanted.
fn available_paths(program: &ast::Program) -> Vec<String> {
    let mut paths = Vec::new();
    for item in &program.items {
        if let ast::Item::FunctionDeclaration(entry) = item
            && entry.name == "^"
        {
            collect_paths(&entry.body, "", &mut paths);
        }
    }
    paths
}

fn collect_paths(expression: &Expression, prefix: &str, paths: &mut Vec<String>) {
    match expression {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            if let Expression::Identifier { name, .. } = function.as_ref()
                && (name == TEST_BLOCK_MARKER || name == TEST_CASE_MARKER)
                && let [Expression::String { value, .. }, body] = arguments.as_slice()
            {
                let path = match prefix.is_empty() {
                    true => value.clone(),
                    false => format!("{prefix}/{value}"),
                };
                paths.push(path.clone());
                if name == TEST_BLOCK_MARKER {
                    collect_paths(body, &path, paths);
                }
                return;
            }
            for argument in arguments {
                collect_paths(argument, prefix, paths);
            }
        }
        Expression::Lambda { body, .. } => collect_paths(body, prefix, paths),
        Expression::Block { statements, .. } => {
            for statement in statements {
                if let Statement::Expression(expression) = statement {
                    collect_paths(expression, prefix, paths);
                }
            }
        }
        _ => {}
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
