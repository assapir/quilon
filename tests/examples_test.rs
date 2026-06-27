//! Examples gate: guarantees every file in `examples/` stays compilable (and, for
//! runnable ones, keeps its documented exit code). Running under `cargo test`, this
//! is the CI gate that stops examples from rotting as the language evolves.

use quilon::ast::Program;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use quilon::{jit, modules};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    ("unit.ql", 0),
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

/// Is a tool available on PATH? (Used to skip the native-AOT gate gracefully when
/// the LLVM/C toolchain genuinely isn't installed — e.g. a minimal dev box.)
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Ensure `libquilon_rt.a` sits next to the `quilon` binary — `quilon build` links
/// it from there. `cargo build --all-targets` (CI) emits it; a bare `cargo test`
/// may not, so build the staticlib on demand.
fn ensure_runtime_lib(bin_dir: &Path) {
    if bin_dir.join("libquilon_rt.a").exists() {
        return;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let _ = Command::new(cargo)
        .args(["build", "-p", "quilon-rt"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status();
}

/// Every runnable example must produce its documented exit code via the in-process
/// JIT (`quilon run`) AND via native AOT (`quilon build`, which emits the object
/// in-process and links) under BOTH linkers (`clang` and `gcc`) — and all paths
/// must agree. This keeps the JIT and the two native link paths from silently
/// diverging (e.g. an intrinsic only the JIT resolves, or a linker-specific break).
/// Skips a linker only if it's genuinely absent on PATH.
#[test]
fn runnable_examples_match_across_jit_and_aot() {
    let linkers: Vec<&str> = ["clang", "gcc"]
        .into_iter()
        .filter(|t| tool_available(t))
        .collect();
    if linkers.is_empty() {
        eprintln!("skipping JIT/AOT parity gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    }
    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    // Unique per process so concurrent `cargo test` invocations never share (and
    // clobber) output binary paths. Cleaned up at the end.
    let tmp = std::env::temp_dir().join(format!("quilon_aot_gate_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    for (name, expected) in EXPECTED_EXIT {
        let src = examples_dir().join(name);

        // In-process JIT.
        let jit = Command::new(quilon)
            .args(["run", src.to_str().unwrap()])
            .output()
            .expect("run quilon run");
        let jit_code = jit.status.code().unwrap_or(-1);
        assert_eq!(jit_code, *expected, "{name}: JIT exit code wrong");

        // Native AOT via each available linker (`quilon build --linker ...`).
        for linker in &linkers {
            let bin = tmp.join(format!("{name}.{linker}"));
            let build = Command::new(quilon)
                .args(["build", src.to_str().unwrap(), "--linker", linker])
                .args(["-o", bin.to_str().unwrap()])
                .output()
                .expect("run quilon build");
            assert!(
                build.status.success(),
                "{name}: `quilon build --linker {linker}` failed: {}",
                String::from_utf8_lossy(&build.stderr)
            );

            let native = Command::new(&bin).output().expect("run native binary");
            let native_code = native.status.code().unwrap_or(-1);
            assert_eq!(
                native_code, *expected,
                "{name}: native AOT ({linker}) exit code wrong"
            );
            assert_eq!(
                native_code, jit_code,
                "{name}: JIT and AOT ({linker}) disagree on exit code"
            );
        }
    }

    // Best-effort cleanup of this run's intermediates.
    let _ = std::fs::remove_dir_all(&tmp);
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
