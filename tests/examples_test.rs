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

fn target_dir() -> PathBuf {
    // Honor CARGO_TARGET_DIR; otherwise it's `<workspace>/target` (the quilon crate
    // is the workspace root, so CARGO_MANIFEST_DIR is the workspace root).
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
}

/// Locate `libquilon_rt.a` (debug or release), building the staticlib if needed.
/// `cargo build --all-targets` (CI) already emits it; locally under a bare
/// `cargo test` it may be absent, so build it on demand.
fn runtime_lib_dir() -> PathBuf {
    let target = target_dir();
    let find = || {
        ["debug", "release"]
            .into_iter()
            .map(|p| target.join(p))
            .find(|d| d.join("libquilon_rt.a").exists())
    };
    if let Some(dir) = find() {
        return dir;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let _ = Command::new(cargo)
        .args(["build", "-p", "quilon-rt"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status();
    find().unwrap_or_else(|| {
        panic!("libquilon_rt.a not found and could not be built (looked under {target:?})")
    })
}

/// Every runnable example must produce its documented exit code via BOTH the
/// in-process JIT (`quilon run`) AND the native ahead-of-time path (compile ->
/// `llc` -> link against libquilon_rt -> run the native binary), and the two must
/// agree. This keeps JIT and AOT from silently diverging (e.g. an intrinsic that
/// only the JIT resolves). Skips only if `llc`/`gcc` are genuinely absent.
#[test]
fn runnable_examples_match_across_jit_and_aot() {
    // clang is the natural linker for LLVM-produced objects; gcc links identically.
    let linker = ["clang", "gcc"].into_iter().find(|t| tool_available(t));
    let (Some(linker), true) = (linker, tool_available("llc")) else {
        eprintln!("skipping JIT/AOT parity gate: need `llc` and a linker (`clang`/`gcc`) on PATH");
        return;
    };
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let rt_dir = runtime_lib_dir();
    // Unique per process so concurrent `cargo test` invocations never share (and
    // clobber) intermediate `.ll`/`.o`/binary paths. Cleaned up at the end.
    let tmp = std::env::temp_dir().join(format!("quilon_aot_gate_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    for (name, expected) in EXPECTED_EXIT {
        let src = examples_dir().join(name);
        let ll = tmp.join(format!("{name}.ll"));
        let obj = tmp.join(format!("{name}.o"));
        let bin = tmp.join(format!("{name}.bin"));

        // Quilon -> LLVM IR (the real CLI compile path).
        let compile = Command::new(quilon)
            .args(["compile", src.to_str().unwrap(), "-o", ll.to_str().unwrap()])
            .output()
            .expect("run quilon compile");
        assert!(
            compile.status.success(),
            "{name}: `quilon compile` failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );

        // IR -> object (PIC, so data relocations link into a PIE binary).
        let llc = Command::new("llc")
            .args(["-relocation-model=pic", "-filetype=obj"])
            .arg(&ll)
            .args(["-o", obj.to_str().unwrap()])
            .status()
            .expect("run llc");
        assert!(llc.success(), "{name}: llc failed");

        // Link against the runtime static lib + Boehm GC + the system libs the
        // Rust staticlib needs.
        let link = Command::new(linker)
            .arg(&obj)
            .args(["-L", rt_dir.to_str().unwrap()])
            .args(["-lquilon_rt", "-lgc", "-lpthread", "-ldl", "-lm"])
            .args(["-o", bin.to_str().unwrap()])
            .output()
            .expect("run gcc");
        assert!(
            link.status.success(),
            "{name}: native link failed: {}",
            String::from_utf8_lossy(&link.stderr)
        );

        // Run the native binary and the JIT; both must equal the documented code.
        let native = Command::new(&bin).output().expect("run native binary");
        let native_code = native.status.code().unwrap_or(-1);

        let jit = Command::new(quilon)
            .args(["run", src.to_str().unwrap()])
            .output()
            .expect("run quilon run");
        let jit_code = jit.status.code().unwrap_or(-1);

        assert_eq!(native_code, *expected, "{name}: native AOT exit code wrong");
        assert_eq!(jit_code, *expected, "{name}: JIT exit code wrong");
        assert_eq!(
            native_code, jit_code,
            "{name}: JIT and AOT disagree on exit code"
        );
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
