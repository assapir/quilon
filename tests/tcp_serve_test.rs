//! End-to-end proof of the raw TCP server layer (`net.@tcpServe`, `Connection`, `Server`).
//!
//! The program under test binds a fixed port, echoes back whatever one connection sends,
//! and stops the server when a second connection sends the word `quit` — the handler
//! reaches the `Server` handle through a top-level atomic global `^` assigns right after
//! `@tcpServe` returns, since a handler is declared (and so must already compile) before
//! the handle exists. `^`'s own block, having called `@tcpServe` directly, does not return
//! until that `kill` has settled the accept loop — so the process's own exit is the proof
//! the whole chain (accept loop, per-connection fiber, kill, block-scope join) completed,
//! under both the in-process JIT (`quilon run`) and a native AOT binary (`quilon build`).

mod common;

use common::ensure_runtime_lib;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A port nothing is bound to right now — bind an ephemeral one and drop it immediately.
/// The same race every other test in this suite that hands a chosen port to a subprocess
/// accepts (`tcp_request_test.rs`'s `closed_address`): vanishingly unlikely on a test box,
/// and the program under test reports a clear bind failure if it ever loses the race.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to find a free port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// The program under test: a `net.@tcpServe` echo server on `port`, stopped by the word
/// `quit` on any connection.
fn program(port: u16) -> String {
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
  server := net.@tcpServe({port}, connection => respond(connection))
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

/// Connect to `port` with a bounded read/write timeout on every op — every client below
/// dials this way, so a bug that leaves a connection open with nothing arriving fails the
/// test in a few seconds instead of hanging the run.
fn connect_with_timeout(port: u16) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

/// Connect to `port`, retrying (bounded) until the server is up — the process under test
/// needs a moment after starting before `@tcpServe` has actually bound and is accepting.
fn connect_once_listening(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match connect_with_timeout(port) {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("never managed to connect to the test server: {error}"),
        }
    }
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

/// Drive the echo-then-quit exchange against a server already listening on `port`.
fn drive_echo_then_quit(port: u16) {
    let mut echoer = connect_once_listening(port);
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

    let mut quitter = connect_with_timeout(port).expect("connect to send quit");
    quitter.write_all(b"quit").expect("write the quit message");
    drop(quitter);
}

#[test]
fn jit_tcp_serve_echoes_then_kill_stops_the_server() {
    let port = free_port();
    let file = temp_ql("jit", &program(port));

    let child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    drive_echo_then_quit(port);

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

    let port = free_port();
    let source = temp_ql("aot", &program(port));
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

    let child = Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the native AOT server binary");

    drive_echo_then_quit(port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "native AOT: the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&binary);
}
