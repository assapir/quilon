//! Checked array indexing, end to end: an invalid index (out of bounds, negative,
//! or NaN) must be a CLEAR runtime error — a message on stderr and exit status 1 —
//! never a raw memory read. The failing path terminates the process, so these tests
//! drive the real `quilon` binary as a subprocess (an in-process JIT run would take
//! the test harness down with it).

use std::io::Write;
use std::process::Command;

/// Write `source` to a temp `.ql` file, `quilon run` it, and return
/// `(exit_code, stderr)`.
fn run(name: &str, source: &str) -> (i32, String) {
    let mut path = std::env::temp_dir();
    path.push(format!("quilon_idx_{}_{}.ql", std::process::id(), name));
    let mut f = std::fs::File::create(&path).expect("create temp .ql");
    f.write_all(source.as_bytes()).expect("write temp .ql");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run quilon");

    let _ = std::fs::remove_file(&path);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn out_of_bounds_index_aborts_with_message() {
    let (code, stderr) = run("oob", "^ = () -> Num => <\n  a = [1, 2, 3]\n  a[10]\n>");
    assert_eq!(code, 1, "OOB index must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains("array index 10 out of bounds (size 3)"),
        "stderr must name the index and size, got: {stderr}"
    );
}

#[test]
fn negative_index_aborts_with_message() {
    let (code, stderr) = run("neg", "^ = () -> Num => <\n  a = [1, 2, 3]\n  a[0 - 1]\n>");
    assert_eq!(code, 1, "negative index must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains("array index -1 out of bounds (size 3)"),
        "stderr must name the index and size, got: {stderr}"
    );
}

#[test]
fn nan_index_aborts_with_message() {
    // Before the check ran on the f64, a NaN index reached `fptosi` — poison, i.e.
    // undefined behavior the moment optimization is enabled.
    let (code, stderr) = run("nan", "^ = () -> Num => <\n  a = [1, 2, 3]\n  a[0 / 0]\n>");
    assert_eq!(code, 1, "NaN index must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains("array index NaN out of bounds (size 3)"),
        "stderr must show the NaN index, got: {stderr}"
    );
}

#[test]
fn in_bounds_and_fractional_indexing_still_work() {
    // A fractional IN-RANGE index truncates toward zero (documented): with one
    // unified f64 Num, index arithmetic like `size / 2` produces fractions.
    let (code, stderr) = run(
        "ok",
        "^ = () -> Num => <\n  a = [10, 20, 30]\n  a[0] + a[1.7] + a[a.size / 2]\n>",
    );
    assert_eq!(code, 50, "10 + 20 + 20 = 50, got {code}: {stderr}");
    assert!(stderr.is_empty(), "no error expected, got: {stderr}");
}

#[test]
fn at_with_invalid_index_returns_notok_instead_of_aborting() {
    // `.at` is the NON-aborting form: NaN and out-of-range map to NotOk. Its bounds
    // check runs on the f64 before conversion, so no poison is involved.
    let (code, stderr) = run(
        "at",
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  bad = a.at(0 / 0) ? | Ok(n) => n | NotOk(_) => 90\n  oob = a.at(9) ? | Ok(n) => n | NotOk(_) => 9\n  bad + oob\n>",
    );
    assert_eq!(code, 99, "NotOk paths must run, got {code}: {stderr}");
}
