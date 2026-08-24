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
        stderr.contains("index 10 out of bounds for an array of size 3"),
        "stderr must name the index and size, got: {stderr}"
    );
}

#[test]
fn negative_index_aborts_with_message() {
    let (code, stderr) = run("neg", "^ = () -> Num => <\n  a = [1, 2, 3]\n  a[0 - 1]\n>");
    assert_eq!(code, 1, "negative index must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains("index -1 out of bounds for an array of size 3"),
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
        stderr.contains("index NaN out of bounds for an array of size 3"),
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

/// The report says WHERE the bad read is: the `arr[i]` expression's own line and column,
/// then the source line with a caret run under the read. (The exact framing is pinned once,
/// across all three renderers, in `tests/fail_loud_location_test.rs`.)
#[test]
fn an_invalid_index_reports_its_own_location() {
    let (code, stderr) = run(
        "located",
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  n = 7\n  a[n]\n>",
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains(":4:3:\nindex 7 out of bounds for an array of size 3"),
        "the report must locate the failing read, got: {stderr}"
    );
    assert!(
        stderr.contains("4 |   a[n]") && stderr.contains("  ^^^^"),
        "the report must show the read with a caret under it, got: {stderr}"
    );
}

/// Two reads in one program report DIFFERENT locations — the point of the change: the
/// report names the read that failed, not just the fact that one did.
#[test]
fn each_read_reports_its_own_line() {
    let src = "^ = () -> Num => <\n  a = [1]\n  n = 3\n  first = a[0]\n  second = a[n]\n  first + second\n>";
    let (_, stderr) = run("which_read", src);
    assert!(
        stderr.contains(":5:12:") && stderr.contains("second = a[n]"),
        "the failing read is on line 5, got: {stderr}"
    );
}

/// A redirected report carries no ANSI escapes: color is for a terminal, so a CI log or a
/// piped build stays plain (the runtime asks the same terminal check `core.test` does).
#[test]
fn a_redirected_report_is_not_colored() {
    let (_, stderr) = run("plain", "^ = () -> Num => <\n  a = [1]\n  a[9]\n>");
    assert!(
        !stderr.contains('\u{1b}'),
        "a non-tty report must not be colored, got: {stderr:?}"
    );
}
