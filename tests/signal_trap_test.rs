//! End-to-end proof of the signal trap (`!>`, `core.process`): a real OS signal, sent to a
//! real running process, runs the matching arm — under both the in-process JIT (`quilon
//! run`) and a native AOT binary (`quilon build`) — and a program with no trap dies with
//! the OS default, untouched.

mod common;

use common::ensure_runtime_lib;
use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A program that prints `ready`, installs an `Interrupt` arm printing `caught`, then
/// sleeps long enough for a signal to arrive and its arm to finish before `^` returns.
const TRAP_PROGRAM: &str = r#"
<< core.process
<< core.io
<< core.time

!> | Interrupt(s) => io.print("caught")

^ = () -> Num => <
  io.print("ready")
  time.@sleep(3)
  0
>
"#;

/// Like [`TRAP_PROGRAM`], but the `Interrupt` arm itself is slow (sleeps) and counts its
/// own runs into an atomic global — proving a burst of arrivals during one run is
/// delivered, at most once, after it returns.
const BURST_PROGRAM: &str = r#"
<< core.process
<< core.io
<< core.time

@runs := 0

onInterrupt = (s :: process.Sender) -> $ => <
  runs := runs + 1
  io.print("run `runs`")
  time.@sleep(1)
>

!> | Interrupt(s) => onInterrupt(s)

^ = () -> Num => <
  io.print("ready")
  time.@sleep(5)
  0
>
"#;

/// A program with no trap at all — SIGTERM must end it with the OS default (no handler
/// installed, no runtime change at all).
const NO_TRAP_PROGRAM: &str = r#"
<< core.io
<< core.time

^ = () -> Num => <
  io.print("ready")
  time.@sleep(5)
  0
>
"#;

/// Write `source` to a unique temp `.qn` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_signal_trap_{tag}_{}_{}.qn",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, source).expect("write temp .qn");
    path
}

/// Wait for `child` to exit, killing it and failing loudly instead of hanging the test run
/// if it does not within `timeout`.
fn wait_bounded(mut child: Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child process") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the test process never exited within {timeout:?} — killed it");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Every line the child prints on stdout, streamed to a channel as it arrives — unlike
/// `common::read_announced_port` (which takes and drops the handle after one line, closing
/// it), this keeps reading for the whole test, since these programs print more than once
/// (`ready`, then whatever their arm prints).
fn stream_stdout_lines(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().expect("the child's stdout is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    receiver
}

/// Wait for a line equal to `expected` within `timeout`, panicking with what actually
/// arrived (if anything) otherwise.
fn expect_line(lines: &mpsc::Receiver<String>, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(remaining) {
            Ok(line) if line == expected => return,
            // Not the line we're waiting for yet — keep reading (e.g. `ready` while
            // waiting for `caught`).
            Ok(_) => {}
            Err(_) => panic!("never saw line {expected:?} within {timeout:?}"),
        }
        if Instant::now() >= deadline {
            panic!("never saw line {expected:?} within {timeout:?}");
        }
    }
}

fn send_signal(pid: u32, signal: i32) {
    // SAFETY: `pid` is this test's own live child process; `kill(2)` with a real signal
    // number just delivers it.
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    assert_eq!(
        result,
        0,
        "kill(2) failed: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn jit_sigint_runs_the_trap_arm() {
    let file = temp_ql("jit", TRAP_PROGRAM);
    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    let pid = child.id();
    let lines = stream_stdout_lines(&mut child);

    expect_line(&lines, "ready", Duration::from_secs(10));
    send_signal(pid, libc::SIGINT);
    expect_line(&lines, "caught", Duration::from_secs(10));

    let status = wait_bounded(child, Duration::from_secs(15));
    assert_eq!(
        status.code(),
        Some(0),
        "the program's own `^` still returns 0 after the arm ran"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_sigint_runs_the_trap_arm() {
    let Some(linker) = ["clang", "gcc"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }) else {
        eprintln!("skipping AOT signal trap gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let source = temp_ql("aot", TRAP_PROGRAM);
    let binary =
        std::env::temp_dir().join(format!("quilon_signal_trap_aot_{}", std::process::id()));
    let build = Command::new(quilon)
        .args(["build", source.to_str().unwrap(), "--linker", linker])
        .args(["-o", binary.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let _ = std::fs::remove_file(&source);

    let mut child = Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the native AOT binary");
    let pid = child.id();
    let lines = stream_stdout_lines(&mut child);

    expect_line(&lines, "ready", Duration::from_secs(10));
    send_signal(pid, libc::SIGINT);
    expect_line(&lines, "caught", Duration::from_secs(10));

    let status = wait_bounded(child, Duration::from_secs(15));
    assert_eq!(
        status.code(),
        Some(0),
        "native AOT: the program's own `^` still returns 0 after the arm ran"
    );
    let _ = std::fs::remove_file(&binary);
}

#[test]
fn a_burst_of_three_signals_runs_the_arm_exactly_twice() {
    let file = temp_ql("burst", BURST_PROGRAM);
    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    let pid = child.id();
    let lines = stream_stdout_lines(&mut child);

    expect_line(&lines, "ready", Duration::from_secs(10));
    // Three arrivals in quick succession, all while the first run's own 1-second sleep is
    // still in progress: at most one stays pending, so exactly two runs follow.
    send_signal(pid, libc::SIGINT);
    std::thread::sleep(Duration::from_millis(50));
    send_signal(pid, libc::SIGINT);
    std::thread::sleep(Duration::from_millis(50));
    send_signal(pid, libc::SIGINT);

    expect_line(&lines, "run 1", Duration::from_secs(10));
    expect_line(&lines, "run 2", Duration::from_secs(10));

    // No third run ever arrives — collect whatever else prints before `^` returns and make
    // sure "run 3" is not among it.
    let mut extra = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while let Ok(line) = lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        extra.push(line);
        if Instant::now() >= deadline {
            break;
        }
    }
    assert!(
        !extra.iter().any(|line| line == "run 3"),
        "a third run must never happen: {extra:?}"
    );

    let status = wait_bounded(child, Duration::from_secs(15));
    assert_eq!(status.code(), Some(0));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_program_without_a_trap_dies_with_the_os_default_on_sigterm() {
    let file = temp_ql("no_trap", NO_TRAP_PROGRAM);
    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    let pid = child.id();
    let lines = stream_stdout_lines(&mut child);

    expect_line(&lines, "ready", Duration::from_secs(10));
    send_signal(pid, libc::SIGTERM);

    let status = wait_bounded(child, Duration::from_secs(10));
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "no trap installed — SIGTERM's OS default (terminate) must apply untouched, got {status:?}"
    );
    let _ = std::fs::remove_file(&file);
}
