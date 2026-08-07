//! `core.test` assertion module: end-to-end exit-code behavior.
//!
//! A passing assertion runs to completion (exit 0); a FAILING assertion prints a
//! message to stderr and exits **101** (the Rust-panic convention `core.test` uses,
//! distinct from the small result codes examples use as their normal exit status).
//!
//! Every case is driven as a SUBPROCESS — `quilon run <file>` (in-process JIT) and,
//! where a linker is present, `quilon build` + execute (native AOT). It must never be
//! the in-process `jit::run_program`, because a failing assertion calls the `__exit`
//! runtime intrinsic (libc `exit`), which would terminate the test runner itself.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const FAIL_CODE: i32 = 101;

fn quilon() -> &'static str {
    env!("CARGO_BIN_EXE_quilon")
}

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quilon_assert_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Write `src` to a uniquely-named `.ql` file under the per-process temp dir.
fn write_program(tag: &str, src: &str) -> PathBuf {
    let path = tmp_dir().join(format!("{tag}.ql"));
    let mut f = std::fs::File::create(&path).expect("create .ql");
    f.write_all(src.as_bytes()).expect("write .ql");
    path
}

/// `(exit_code, stderr)` from `quilon run <file>` (in-process JIT, as a subprocess).
fn run_jit(tag: &str, src: &str) -> (i32, String) {
    let path = write_program(tag, src);
    let out = Command::new(quilon())
        .args(["run", path.to_str().unwrap()])
        .output()
        .expect("spawn quilon run");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Is a linker available on PATH? (Mirrors the examples gate's graceful skip.)
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Ensure a FRESH `libquilon_rt.a` sits next to the `quilon` binary so `quilon build`
/// links the just-built runtime (which must contain `__exit`) rather than a stale
/// cached archive. Same subtlety as the examples gate — see its `ensure_runtime_lib`.
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
        "failed to build libquilon_rt.a for the native-AOT assert gate"
    );
    let fresh = rt_target.join("debug").join("libquilon_rt.a");
    std::fs::copy(&fresh, bin_dir.join("libquilon_rt.a"))
        .expect("copy fresh libquilon_rt.a next to the quilon binary");
}

/// `(exit_code, stderr)` from a native AOT build (`quilon build --linker <linker>`)
/// then executing the resulting binary.
fn run_aot(tag: &str, src: &str, linker: &str) -> (i32, String) {
    let path = write_program(tag, src);
    let bin = tmp_dir().join(format!("{tag}.{linker}.bin"));
    let build = Command::new(quilon())
        .args(["build", path.to_str().unwrap(), "--linker", linker])
        .args(["-o", bin.to_str().unwrap()])
        .output()
        .expect("spawn quilon build");
    assert!(
        build.status.success(),
        "{tag}: `quilon build --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&bin).output().expect("run native binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// --- The primitive: assert -------------------------------------------------

#[test]
fn passing_assert_exits_zero() {
    let (code, _) = run_jit(
        "assert_pass",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 2)\n",
    );
    assert_eq!(code, 0, "a passing assert must exit 0");
}

#[test]
fn failing_assert_exits_101_with_stderr_message() {
    let (code, stderr) = run_jit(
        "assert_fail",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 3)\n",
    );
    assert_eq!(code, FAIL_CODE, "a failing assert must exit {FAIL_CODE}");
    assert!(
        stderr.contains("assertion failed"),
        "failing assert must print the default message to stderr, got: {stderr:?}"
    );
}

#[test]
fn passing_assert_with_message_exits_zero() {
    // The `AssertOpts` message overload: a holding condition still does nothing.
    let (code, _) = run_jit(
        "assert_msg_pass",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 2, AssertOpts { message = \"unused\" })\n",
    );
    assert_eq!(code, 0, "a passing assert(cond, opts) must exit 0");
}

#[test]
fn failing_assert_with_message_prints_that_message() {
    // The `AssertOpts` message overload prints opts.message (not the default) and
    // still exits 101.
    let (code, stderr) = run_jit(
        "assert_msg_fail",
        "<< core.test\n^ = () -> $ => assert(1 + 1 == 3, AssertOpts { message = \"custom boom\" })\n",
    );
    assert_eq!(
        code, FAIL_CODE,
        "a failing assert(cond, opts) must exit {FAIL_CODE}"
    );
    assert!(
        stderr.contains("custom boom"),
        "the message overload must print opts.message to stderr, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("assertion failed"),
        "the message overload must NOT print the default message, got: {stderr:?}"
    );
}

// --- assertEq / assertNotEq ------------------------------------------------

#[test]
fn assert_eq_pass_and_fail() {
    // Num.
    let (code, _) = run_jit(
        "eq_num_pass",
        "<< core.test\n^ = () -> $ => assertEq(6 * 7, 42)\n",
    );
    assert_eq!(code, 0);

    let (code, stderr) = run_jit(
        "eq_num_fail",
        "<< core.test\n^ = () -> $ => assertEq(6 * 7, 41)\n",
    );
    assert_eq!(code, FAIL_CODE);
    // Failure message renders expected (41) and actual (42) via eprint.
    assert!(
        stderr.contains("41") && stderr.contains("42"),
        "assertEq failure must show expected vs actual, got: {stderr:?}"
    );

    // Text and Bool overloads.
    let (code, _) = run_jit(
        "eq_text_pass",
        "<< core.test\n^ = () -> $ => assertEq(\"a\" + \"b\", \"ab\")\n",
    );
    assert_eq!(code, 0);
    let (code, _) = run_jit(
        "eq_bool_fail",
        "<< core.test\n^ = () -> $ => assertEq(1 < 2, false)\n",
    );
    assert_eq!(code, FAIL_CODE);
}

#[test]
fn assert_not_eq_pass_and_fail() {
    let (code, _) = run_jit(
        "ne_pass",
        "<< core.test\n^ = () -> $ => assertNotEq(1, 2)\n",
    );
    assert_eq!(code, 0);

    let (code, stderr) = run_jit(
        "ne_fail",
        "<< core.test\n^ = () -> $ => assertNotEq(5, 5)\n",
    );
    assert_eq!(code, FAIL_CODE);
    assert!(stderr.contains("assertion failed"), "got: {stderr:?}");
}

// --- assertOk / assertNotOk ------------------------------------------------

#[test]
fn assert_ok_pass_and_fail() {
    let (code, _) = run_jit(
        "ok_pass",
        "<< core.test\n^ = () -> $ => assertOk([10, 20].at(0))\n",
    );
    assert_eq!(code, 0);

    let (code, _) = run_jit(
        "ok_fail",
        "<< core.test\n^ = () -> $ => assertOk([10, 20].at(9))\n",
    );
    assert_eq!(code, FAIL_CODE);
}

#[test]
fn assert_not_ok_pass_and_fail() {
    let (code, _) = run_jit(
        "notok_pass",
        "<< core.test\n^ = () -> $ => assertNotOk([10, 20].at(9))\n",
    );
    assert_eq!(code, 0);

    let (code, _) = run_jit(
        "notok_fail",
        "<< core.test\n^ = () -> $ => assertNotOk([10, 20].at(0))\n",
    );
    assert_eq!(code, FAIL_CODE);
}

// --- Native AOT parity -----------------------------------------------------

/// The exit-code contract must also hold for native AOT binaries (both linkers): a
/// passing assert exits 0, a failing one exits 101 with a stderr message. This is the
/// path that would expose a missing `__exit` runtime symbol (the JIT maps symbols by
/// address and could mask an AOT link failure).
#[test]
fn native_aot_assert_exit_codes() {
    let linkers: Vec<&str> = ["clang", "gcc"]
        .into_iter()
        .filter(|t| tool_available(t))
        .collect();
    if linkers.is_empty() {
        eprintln!("skipping native-AOT assert gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    }
    ensure_runtime_lib(
        Path::new(quilon())
            .parent()
            .expect("binary has a parent dir"),
    );

    for linker in &linkers {
        let (code, _) = run_aot(
            &format!("aot_pass_{linker}"),
            "<< core.test\n^ = () -> $ => assertEq(2 + 2, 4)\n",
            linker,
        );
        assert_eq!(code, 0, "native AOT ({linker}): passing assert must exit 0");

        let (code, stderr) = run_aot(
            &format!("aot_fail_{linker}"),
            "<< core.test\n^ = () -> $ => assertEq(2 + 2, 5)\n",
            linker,
        );
        assert_eq!(
            code, FAIL_CODE,
            "native AOT ({linker}): failing assert must exit {FAIL_CODE}"
        );
        assert!(
            stderr.contains("assertion failed"),
            "native AOT ({linker}): failing assert must print to stderr, got: {stderr:?}"
        );

        // Message overload (AssertOpts) across the native path: prints opts.message.
        let (code, stderr) = run_aot(
            &format!("aot_msg_{linker}"),
            "<< core.test\n^ = () -> $ => assert(1 == 2, AssertOpts { message = \"aot boom\" })\n",
            linker,
        );
        assert_eq!(
            code, FAIL_CODE,
            "native AOT ({linker}): failing assert(cond, opts) must exit {FAIL_CODE}"
        );
        assert!(
            stderr.contains("aot boom"),
            "native AOT ({linker}): message overload must print opts.message, got: {stderr:?}"
        );
    }
}
