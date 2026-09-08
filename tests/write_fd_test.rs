//! `io.write`'s file descriptor, end to end: a computed `fd` that is not a whole number of
//! 0 or more must fail loud (QN508) rather than reach the OS as poison. The failing path
//! terminates the process, so these tests drive the real `quilon` binary as a subprocess
//! (an in-process JIT run would take the test harness down with it).

use std::io::Write;
use std::process::Command;

/// Write `source` to a temp `.qn` file, `quilon run` it, and return `(exit_code, stderr)`.
fn run(name: &str, source: &str) -> (i32, String) {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_write_fd_{}_{}.qn",
        std::process::id(),
        name
    ));
    let mut file = std::fs::File::create(&path).expect("create temp .qn");
    file.write_all(source.as_bytes()).expect("write temp .qn");

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
fn a_nan_fd_aborts_with_qn508() {
    let (code, stderr) = run(
        "nan",
        "<< core.io\n\
         ^ = () -> Num => <\n  nonsenseFd = 0 / 0\n  io.write(\"raccoon telegram\", nonsenseFd)\n>\n",
    );
    assert_eq!(code, 1, "a NaN fd must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains(
            "error[QN508]: write: file descriptor must be a whole number of 0 or more, got NaN"
        ),
        "stderr must name the code and the NaN fd, got: {stderr}"
    );
}

#[test]
fn a_negative_fd_aborts_with_qn508() {
    let (code, stderr) = run(
        "negative",
        "<< core.io\n\
         ^ = () -> Num => <\n  wrongWayFd = 0 - 1\n  io.write(\"raccoon telegram\", wrongWayFd)\n>\n",
    );
    assert_eq!(code, 1, "a negative fd must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains(
            "error[QN508]: write: file descriptor must be a whole number of 0 or more, got -1"
        ),
        "stderr must name the code and the negative fd, got: {stderr}"
    );
}

#[test]
fn a_fractional_fd_aborts_with_qn508() {
    let (code, stderr) = run(
        "fractional",
        "<< core.io\n\
         ^ = () -> Num => <\n  halfOpenFd = 1 / 2\n  io.write(\"raccoon telegram\", halfOpenFd)\n>\n",
    );
    assert_eq!(code, 1, "a fractional fd must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains(
            "error[QN508]: write: file descriptor must be a whole number of 0 or more, got 0.5"
        ),
        "stderr must name the code and the fractional fd, got: {stderr}"
    );
}

#[test]
fn an_infinite_fd_aborts_with_qn508() {
    let (code, stderr) = run(
        "infinite",
        "<< core.io\n\
         ^ = () -> Num => <\n  boundlessFd = 1 / 0\n  io.write(\"raccoon telegram\", boundlessFd)\n>\n",
    );
    assert_eq!(code, 1, "an infinite fd must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains(
            "error[QN508]: write: file descriptor must be a whole number of 0 or more, got inf"
        ),
        "stderr must name the code and the infinite fd, got: {stderr}"
    );
}

#[test]
fn a_whole_non_negative_fd_still_writes() {
    let (code, stderr) = run(
        "ok",
        "<< core.io\n\
         ^ = () -> Num => < io.write(\"raccoon telegram\", io.stdout) - 16 >\n",
    );
    assert_eq!(
        code, 0,
        "a valid fd must write normally, got exit {code}: {stderr}"
    );
}
