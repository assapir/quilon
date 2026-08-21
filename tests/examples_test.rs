//! Examples gate: guarantees every file in `examples/` stays compilable, and that
//! every runnable one is SELF-ASSERTING — it verifies its own results in-language
//! (via `<< core.test`) and exits 0. Running under `cargo test`, this is the CI gate
//! that stops examples from rotting as the language evolves.

use quilon::driver::front_end;
use quilon::jit;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::JIT_LOCK;
use common::ensure_runtime_lib;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// Examples that are intentionally rejected by the compiler (negative examples).
const EXPECT_COMPILE_ERROR: &[&str] = &["type_error.ql", "global_computed.ql"];

/// Examples that compile and run but deliberately FAIL, because the failure IS what they
/// demonstrate: `(file, exit code, a fragment its stderr must contain)`. They are excluded
/// from the exit-0 gates below and checked by `failing_examples_report_their_failure`.
const EXPECT_RUNTIME_FAILURE: &[(&str, i32, &str)] = &[
    (
        "assert_location.ql",
        101,
        "assert_location.ql:23:3: assertion failed: expected 42, got 41",
    ),
    (
        "index_out_of_bounds.ql",
        1,
        "index_out_of_bounds.ql:21:11: index 7 out of bounds for an array of size 3",
    ),
];

/// Whether `name` is one of the deliberately-failing examples.
fn expects_runtime_failure(name: &str) -> bool {
    EXPECT_RUNTIME_FAILURE.iter().any(|(f, _, _)| *f == name)
}

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

/// The runnable examples: every `.ql` that defines `^`, is not a negative example, and is
/// not one of the deliberately-failing ones. A new runnable example is picked up
/// automatically — no per-file registration.
fn runnable_examples() -> Vec<PathBuf> {
    ql_files()
        .into_iter()
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            !EXPECT_COMPILE_ERROR.contains(&name.as_str())
                && !expects_runtime_failure(&name)
                && defines_entry(p)
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
        } else if let Err(e) = result {
            // `FrontEndError` renders as the diagnostic the CLI would print.
            panic!("{name} failed to compile: {e}");
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
/// Point the process's stdin at `/dev/null` so an example that reads stdin (`@readStdin`)
/// sees end-of-input immediately and returns `""` instead of blocking on a live terminal.
/// The examples run in-process (below), so this must be the real fd 0. `/dev/null` reads as
/// instant EOF and — being non-pollable — never reaches the reactor, so no fiber ever parks
/// on it. Harmless for every other example (none reads stdin). Best-effort: if `/dev/null`
/// can't be opened, the run simply proceeds with the inherited stdin.
fn silence_stdin() {
    // SAFETY: `open`/`dup2` on standard descriptors; a failure is ignored (best-effort).
    unsafe {
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
        if devnull >= 0 {
            libc::dup2(devnull, 0);
            if devnull != 0 {
                libc::close(devnull);
            }
        }
    }
}

#[test]
fn runnable_examples_exit_zero() {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // Examples run in-process, so guarantee EOF stdin here (an example may `@readStdin`).
    silence_stdin();
    for path in runnable_examples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let checked = front_end(&path).unwrap_or_else(|e| panic!("{name} failed to compile: {e}"));
        let code = jit::run_program(
            &checked.program,
            checked.types,
            checked.defer,
            checked.sources,
            &["program".to_string()],
        )
        .unwrap_or_else(|e| panic!("{name} failed to run: {e}"));
        assert_eq!(code, 0, "{name}: self-asserting example did not exit 0");
    }
}

/// The deliberately-failing examples: each must exit with its documented code and print
/// its documented location to stderr. This is what keeps a failure-demonstrating example
/// honest — its own header shows the report, and here the report is checked against it.
///
/// Driven as a SUBPROCESS (`quilon run`), never the in-process JIT: a failing assertion
/// calls `__exit`, which would terminate the test runner.
#[test]
fn failing_examples_report_their_failure() {
    let quilon = env!("CARGO_BIN_EXE_quilon");
    for (name, code, expected) in EXPECT_RUNTIME_FAILURE {
        let path = examples_dir().join(name);
        assert!(path.exists(), "{name} is registered but missing");

        let out = Command::new(quilon)
            .args(["run", path.to_str().unwrap()])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("run quilon run");
        assert_eq!(
            out.status.code().unwrap_or(-1),
            *code,
            "{name}: expected exit {code}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(expected),
            "{name}: stderr must contain {expected:?}, got: {stderr:?}"
        );
        // The example's own header documents the report; a drifted line number there is a
        // stale example, so the documented text must appear in the file too.
        let src = std::fs::read_to_string(&path).expect("read example");
        assert!(
            src.contains(expected),
            "{name}: its header must document the report it prints ({expected:?})"
        );
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

        // In-process JIT: every self-asserting example exits 0. Feed EOF stdin so an example
        // that reads stdin (`@readStdin`) returns `""` immediately instead of blocking.
        let jit = Command::new(quilon)
            .args(["run", src.to_str().unwrap()])
            .stdin(std::process::Stdio::null())
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

            let native = Command::new(&bin)
                .stdin(std::process::Stdio::null())
                .output()
                .expect("run native binary");
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
