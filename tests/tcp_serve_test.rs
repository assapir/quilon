//! End-to-end proof of the raw TCP server layer (`net.@tcpServe`, `Connection`, `Server`).
//!
//! The program under test binds port `0` and prints the port it bound as the first line of
//! its own stdout. It echoes back whatever one connection sends, and stops the server when
//! a second connection sends the word `quit` — the handler reaches the `Server` handle
//! through a top-level atomic global `^` assigns right after `@tcpServe` returns, since a
//! handler is declared (and so must already compile) before the handle exists. `^`'s own
//! block, having called `@tcpServe` directly, does not return until that `kill` has settled
//! the accept loop — so the process's own exit is the proof the whole chain (accept loop,
//! per-connection fiber, kill, block-scope join) completed, under both the in-process JIT
//! (`quilon run`) and a native AOT binary (`quilon build`).

mod common;

use common::{connect_with_timeout, ensure_runtime_lib, read_announced_port};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The program under test: a `net.@tcpServe` echo server bound to `address` (`host:port`,
/// port `0` in every test below), printing the port it bound as the first line of its own
/// stdout, then stopped by the word `quit` on any connection.
fn program(address: &str) -> String {
    format!(
        r#"
<< core.net
<< core.io

@server := net.Server {{ handle = 0 }}

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit" ? server.kill(1) : connection.@write(line)
  $
>

^ = () -> Num => <
  server := net.@tcpServe("{address}", connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

/// Write `source` to a unique temp `.qn` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_tcp_serve_{tag}_{}_{}.qn",
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
/// if it does not within `timeout` — every regression this test could catch is a stuck
/// server or a `kill` that never settles, both of which must show up as a clear failure,
/// never as `cargo test` never returning.
fn wait_bounded(mut child: Child, timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child process") {
            return status.code().expect("process exited with a code");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the test server never exited within {timeout:?} — killed it");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drive the echo-then-quit exchange against a server already listening on `host:port`.
fn drive_echo_then_quit(host: &str, port: u16) {
    let mut echoer = connect_with_timeout(host, port).expect("connect to the test server");
    echoer
        .write_all(b"knock knock")
        .expect("write the echo message");
    let mut echoed = vec![0u8; b"knock knock".len()];
    echoer
        .read_exact(&mut echoed)
        .expect("read the echoed bytes");
    assert_eq!(
        &echoed, b"knock knock",
        "the server echoed the real bytes back"
    );
    drop(echoer);

    let mut quitter = connect_with_timeout(host, port).expect("connect to send quit");
    quitter.write_all(b"quit").expect("write the quit message");
    drop(quitter);
}

#[test]
fn jit_tcp_serve_echoes_then_kill_stops_the_server() {
    let file = temp_ql("jit", &program("127.0.0.1:0"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    let port = read_announced_port(&mut child);

    drive_echo_then_quit("127.0.0.1", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_tcp_serve_binds_a_hostname() {
    // `net.@tcpServe` resolves a hostname on the runtime's blocking-call pool exactly the
    // way `net.@tcpRequest` does — "localhost" is the one every machine running this test
    // resolves without a real network, and consistently between the server's own bind and
    // this test's client connect (both run on the same machine, through the same resolver).
    let file = temp_ql("jit_hostname", &program("localhost:0"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    let port = read_announced_port(&mut child);

    drive_echo_then_quit("localhost", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_tcp_serve_echoes_then_kill_stops_the_server() {
    let Some(linker) = ["clang", "gcc"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }) else {
        eprintln!("skipping AOT @tcpServe gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let source = temp_ql("aot", &program("127.0.0.1:0"));
    let binary = std::env::temp_dir().join(format!("quilon_tcp_serve_aot_{}", std::process::id()));
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
        .expect("spawn the native AOT server binary");
    let port = read_announced_port(&mut child);

    drive_echo_then_quit("127.0.0.1", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "native AOT: the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&binary);
}
