//! `write`'s fail-loud path when the reader goes away: a program piped into something that
//! stops reading must report `QN509` and exit 5 — never run quietly to exit 0 (the JIT's
//! own pre-existing bug: the host process ignores `SIGPIPE`, so a dropped write looked like
//! success) and never die to a bare, message-less `SIGPIPE` (the native pre-existing bug:
//! nothing ignored it there at all).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

mod common;
use common::{ensure_runtime_lib, tool_available};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A program that floods stdout with far more lines than a pipe can hold unread, so a
/// reader that closes early is certain to still be mid-stream when its next write lands.
const CHATTY_PROGRAM: &str = "<< core.io\n\
     shout = (n :: Num) -> Num => <\n  io.print(\"still shouting into the void\")\n  n <= 1 ? 0 : shout(n - 1)\n>\n\
     ^ = () -> Num => < shout(200000) >\n";

fn temp_program(tag: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("quilon_pipe_fail_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{tag}.qn"));
    std::fs::write(&path, CHATTY_PROGRAM).expect("write temp program");
    path
}

/// Spawn `command` with stdout and stderr piped, read a few bytes from stdout, then drop
/// the read end while the child is almost certainly still writing, and return `(exit
/// code, stderr)`.
fn run_and_close_reader_early(mut command: Command) -> (Option<i32>, String) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut sip = [0u8; 64];
    let _ = stdout.read(&mut sip);
    drop(stdout); // closes our read end; the child's next write now fails with EPIPE
    let output = child.wait_with_output().expect("wait for child");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn quilon_run_reports_a_broken_pipe_instead_of_finishing_quietly() {
    let program = temp_program("jit");
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command.args(["run", program.to_str().unwrap()]);

    let (code, stderr) = run_and_close_reader_early(command);
    assert_eq!(
        code,
        Some(5),
        "a broken pipe must exit 5, got {code:?}: {stderr}"
    );
    assert!(
        stderr.contains("error[QN509]: write failed: Broken pipe (EPIPE)"),
        "stderr must name the new code and EPIPE, got: {stderr}"
    );
}

#[test]
fn native_binary_reports_a_broken_pipe_instead_of_dying_to_sigpipe() {
    let linker = ["clang", "gcc"].into_iter().find(|t| tool_available(t));
    let Some(linker) = linker else {
        eprintln!("skipping native broken-pipe gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    ensure_runtime_lib(quilon.parent().expect("the compiler's directory"));

    let program = temp_program("native");
    let binary = program.with_extension("");
    let build = Command::new(&quilon)
        .args(["build", program.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["-o".as_ref(), binary.as_os_str()])
        .output()
        .expect("spawn quilon build");
    assert!(
        build.status.success(),
        "quilon build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let (code, stderr) = run_and_close_reader_early(Command::new(&binary));
    assert_eq!(
        code,
        Some(5),
        "a broken pipe must exit 5 in a native binary too, got {code:?}: {stderr}"
    );
    assert!(
        stderr.contains("error[QN509]: write failed: Broken pipe (EPIPE)"),
        "stderr must name the new code and EPIPE, got: {stderr}"
    );
}
