//! Examples gate: guarantees every file in `examples/` stays compilable, and that
//! every runnable one is SELF-ASSERTING — it verifies its own results in-language
//! (via `<< core.test`) and exits 0. Running under `cargo test`, this is the CI gate
//! that stops examples from rotting as the language evolves.

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

fn ql_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("examples/ should exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ql"))
        .collect();
    files.sort();
    files
}

/// A file defines an entry point (`^`) iff some line starts with `^`.
fn defines_entry(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|src| src.lines().any(|l| l.trim_start().starts_with("^")))
        .unwrap_or(false)
}

/// The runnable examples: every `.ql` that defines `^` and is not a negative example.
/// A new runnable example is picked up automatically — no per-file registration.
fn runnable_examples() -> Vec<PathBuf> {
    ql_files()
        .into_iter()
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            !EXPECT_COMPILE_ERROR.contains(&name.as_str()) && defines_entry(p)
        })
        .collect()
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

/// The self-asserting contract, enforced statically: every runnable example must
/// import `<< core.test` and actually call an `assert*` helper. Exiting 0 alone is a
/// weak signal (a program with no checks trivially passes), so this keeps every
/// example genuinely verifying its own results — the invariant the docs promise.
#[test]
fn every_runnable_example_self_asserts() {
    for path in runnable_examples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).unwrap();
        assert!(
            src.contains("<< core.test"),
            "{name} is runnable but does not import `<< core.test` (not self-asserting)"
        );
        assert!(
            src.contains("assert"),
            "{name} imports core.test but never calls an `assert*` helper"
        );
    }
}

/// Every runnable example is self-asserting: it exits 0 under the in-process JIT.
/// (A failed in-language assertion exits 101, so any regression fails here.)
#[test]
fn runnable_examples_exit_zero() {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for path in runnable_examples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let program = front_end(&path).unwrap_or_else(|e| panic!("{name} failed to compile: {e}"));
        let code = jit::run_program(&program, &["program".to_string()])
            .unwrap_or_else(|e| panic!("{name} failed to run: {e}"));
        assert_eq!(code, 0, "{name}: self-asserting example did not exit 0");
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

/// Ensure a FRESH `libquilon_rt.a` sits next to the `quilon` binary — `quilon build`
/// links it from there. This is subtle:
///
/// - Neither `cargo test` nor `cargo build --all-targets` emits the `staticlib`
///   artifact (nothing in those target sets *consumes* it as a staticlib), so the `.a`
///   in `target/` is whatever a previous build left — and in CI that is a STALE copy
///   restored from the build cache. That is exactly how a newly-added runtime intrinsic
///   (`__text_cmp`) links under the JIT yet fails AOT with `undefined reference`: the
///   program references it, but the cached `.a` predates it.
/// - Simply re-running `cargo build -p quilon-rt` does NOT help: if the crate's
///   fingerprint is already up to date (the rlib was compiled this run), cargo will not
///   re-emit the staticlib output, even if the `.a` on disk is stale/missing.
///
/// So build `quilon-rt` into a DEDICATED, cache-free target dir (which forces a fresh
/// staticlib emit every time) and copy that `.a` next to the `quilon` binary.
fn ensure_runtime_lib(bin_dir: &Path) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let rt_target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("rt-staticlib");
    let status = Command::new(&cargo)
        .args(["build", "-p", "quilon-rt"])
        .arg("--target-dir")
        .arg(&rt_target)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status();
    assert!(
        status.is_ok_and(|s| s.success()),
        "failed to build libquilon_rt.a for the native-AOT gate"
    );
    let fresh = rt_target.join("debug").join("libquilon_rt.a");
    std::fs::copy(&fresh, bin_dir.join("libquilon_rt.a"))
        .expect("copy fresh libquilon_rt.a next to the quilon binary");
}

/// Every runnable example must exit 0 via the in-process JIT (`quilon run`) AND via
/// native AOT (`quilon build`, which emits the object in-process and links) under
/// BOTH linkers (`clang` and `gcc`) — and all paths must agree. This keeps the JIT
/// and the two native link paths from silently diverging (e.g. an intrinsic only the
/// JIT resolves, or a linker-specific break). A failed in-language assertion exits
/// 101, so a broken example fails the gate naturally.
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

    for src in runnable_examples() {
        let name = src.file_name().unwrap().to_string_lossy().to_string();

        // In-process JIT: every self-asserting example exits 0.
        let jit = Command::new(quilon)
            .args(["run", src.to_str().unwrap()])
            .output()
            .expect("run quilon run");
        let jit_code = jit.status.code().unwrap_or(-1);
        assert_eq!(jit_code, 0, "{name}: JIT exit code wrong (expected 0)");

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
                native_code, 0,
                "{name}: native AOT ({linker}) exit code wrong (expected 0)"
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
