//! Examples gate: guarantees every file in `examples/` stays compilable (and, for
//! runnable ones, keeps its documented exit code). Running under `cargo test`, this
//! is the CI gate that stops examples from rotting as the language evolves.

use quilon::ast::Program;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use quilon::{jit, modules};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// LLVM JIT / target init isn't thread-safe; cargo runs tests in parallel.
static JIT_LOCK: Mutex<()> = Mutex::new(());

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// The full front-end (read -> lex -> parse -> resolve `<<` imports -> typecheck),
/// returning the import-linked program. Mirrors `driver::front_end` (which lives in
/// the binary, not the lib crate).
fn front_end(path: &Path) -> Result<Program, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let tokens = Lexer::tokenize(&src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let linked = modules::link(program, base).map_err(|e| format!("import: {e}"))?;
    TypeChecker::new()
        .check_program(&linked)
        .map_err(|e| format!("type: {e}"))?;
    Ok(linked)
}

/// Examples that are intentionally rejected by the compiler (negative examples).
const EXPECT_COMPILE_ERROR: &[&str] = &["type_error.ql"];

/// Runnable examples (define `^`) and their documented exit codes.
const EXPECTED_EXIT: &[(&str, i32)] = &[
    ("hello_world.ql", 42),
    ("arithmetic.ql", 12),
    ("factorial.ql", 120),
    ("fibonacci.ql", 55),
    ("pattern_match.ql", 50),
    ("arrays.ql", 5),
    ("for_loop.ql", 0),
    ("pipeline.ql", 25),
    ("text.ql", 7),
    ("io.ql", 0),
    ("records.ql", 28),
    ("methods.ql", 35),
    ("result.ql", 84),
    ("use_module.ql", 5),
];

fn ql_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("examples/ should exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ql"))
        .collect();
    files.sort();
    files
}

/// Every `.ql` in examples/ must either compile, or (if a known negative) fail to.
/// This is the gate: a new example is covered automatically.
#[test]
fn all_examples_compile() {
    for path in ql_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let result = front_end(&path);
        if EXPECT_COMPILE_ERROR.contains(&name.as_str()) {
            assert!(
                result.is_err(),
                "{name} is a negative example but compiled cleanly"
            );
        } else {
            assert!(
                result.is_ok(),
                "{name} failed to compile: {:?}",
                result.err()
            );
        }
    }
}

/// Every runnable example produces its documented exit code via the JIT.
#[test]
fn runnable_examples_have_expected_exit_codes() {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (name, expected) in EXPECTED_EXIT {
        let path = examples_dir().join(name);
        let program = front_end(&path).unwrap_or_else(|e| panic!("{name} failed to compile: {e}"));
        let code =
            jit::run_program(&program).unwrap_or_else(|e| panic!("{name} failed to run: {e}"));
        assert_eq!(code, *expected, "{name}: wrong exit code");
    }
}

/// Keep the exit-code table honest: every runnable example (one defining `^`, and
/// not a negative) must be listed in EXPECTED_EXIT, so none silently goes unrun.
#[test]
fn every_runnable_example_is_listed() {
    for path in ql_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if EXPECT_COMPILE_ERROR.contains(&name.as_str()) {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let defines_entry = src.lines().any(|l| l.trim_start().starts_with("^"));
        let listed = EXPECTED_EXIT.iter().any(|(n, _)| *n == name);
        if defines_entry {
            assert!(
                listed,
                "{name} defines `^` but is missing from EXPECTED_EXIT"
            );
        }
    }
}
