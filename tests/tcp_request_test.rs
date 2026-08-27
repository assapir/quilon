//! End-to-end proof of the internal `@tcpRequest` request-exchange socket primitive.
//!
//! `@tcpRequest(address, requestBytes)` launches a background TCP exchange — connect, write the
//! request, read the response until the peer closes — and returns a DEFERRED `Result`
//! (`Ok(responseBytes)` on success, `NotOk(errorMessage)` on any network failure); the deferred
//! value flows through the program and is FORCED where a strict operation reads it (the `?` match).
//! These tests spawn a tiny LOCAL listener on a background thread, then compile and run a program
//! that does one `@tcpRequest` against it — proving the response bytes flowed back and forced
//! correctly on both the in-process JIT (`quilon run`) and, when a linker is present, a native AOT
//! binary (`quilon build`). A separate test dials a closed port and proves a failure comes back as
//! `NotOk` for the program to match — never a process crash.

mod common;

use common::ensure_runtime_lib;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;

/// A program that sends `PING\n` to `address`, matches the `Result`, and asserts the `Ok`
/// response equals `expected`. The deferred `@tcpRequest` value is forced at the `?` match: a
/// matching response exits 0, a mismatch trips the assertion (exit 101) — which proves the REAL
/// response bytes (not a constant) reached the compare — and a `NotOk` fails outright, which
/// would flag a connection that should have succeeded.
fn program(address: &str, expected: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.net

^ = () -> Num => <
  @tcpRequest("{address}", "PING\n") ?
    | Ok(response) => assert(response, equals("{expected}"))
    | NotOk(error) => failAt(error)
  0
>
"#
    )
}

/// A program that dials `address` (expected to refuse the connection) and passes ONLY if the
/// exchange comes back as `NotOk` — a `Result` the program matches, not a crash. An `Ok` fails the
/// assertion (exit 101), proving the failure is delivered as a value.
fn failure_program(address: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.net

^ = () -> Num => <
  @tcpRequest("{address}", "PING\n") ?
    | Ok(_)      => failAt("expected a connection failure, got Ok")
    | NotOk(_)   => $
  0
>
"#
    )
}

/// Bind a loopback listener, take its address, then drop it — so the address is one nothing is
/// listening on, and a connect to it is refused. The standard way to get a reliably-closed port.
fn closed_address() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to find a free port");
    let address = listener.local_addr().expect("local addr").to_string();
    drop(listener);
    address
}

/// Bind a loopback listener and serve exactly `connections` request exchanges on a background
/// thread: read the request, write the fixed `PONG\n` response, then close (dropping the stream
/// closes the connection, which is what ends the client's read-to-close). Returns the `host:port`
/// address to dial and the server thread's handle.
fn spawn_pong_server(connections: usize) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local listener");
    let address = listener.local_addr().expect("local addr").to_string();
    let handle = std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut conn, _) = listener.accept().expect("accept a connection");
            // Read the request before responding, so closing the socket sends no reset.
            let mut buffer = [0u8; 64];
            let _ = conn.read(&mut buffer);
            conn.write_all(b"PONG\n").expect("write the response");
        }
    });
    (address, handle)
}

/// Write `source` to a unique temp `.qn` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_tcp_{tag}_{}_{}.qn",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, source).expect("write temp .qn");
    path
}

/// Run `command` to completion and return its exit code.
fn run(mut command: Command) -> Option<i32> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run subprocess")
        .code()
}

/// `quilon run <file>` (in-process JIT).
fn jit_run(file: &Path) -> Option<i32> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command.args(["run", file.to_str().unwrap()]);
    run(command)
}

/// The first available linker (`clang`, then `gcc`), or `None` to skip the AOT half.
fn available_linker() -> Option<&'static str> {
    ["clang", "gcc"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    })
}

#[test]
fn jit_tcp_request_round_trips_and_forces() {
    // Two connections: a matching-response run (exits 0) and a non-matching one (trips the
    // assertion, exit 101) — the latter proving the forced value is the server's real bytes.
    let (address, server) = spawn_pong_server(2);

    let match_file = temp_ql("match", &program(&address, "PONG\\n"));
    assert_eq!(
        jit_run(&match_file),
        Some(0),
        "@tcpRequest should force to the server's \"PONG\" response and pass"
    );

    let mismatch_file = temp_ql("mismatch", &program(&address, "NOPE\\n"));
    assert_eq!(
        jit_run(&mismatch_file),
        Some(101),
        "a non-matching expectation must trip the assertion, proving the real bytes flowed"
    );

    server.join().expect("server thread");
    let _ = std::fs::remove_file(&match_file);
    let _ = std::fs::remove_file(&mismatch_file);
}

#[test]
fn jit_tcp_request_failure_returns_not_ok() {
    // Dialing a closed port must NOT crash the process: the exchange returns `NotOk`, the program
    // matches it and exits 0. An `Ok` (or a process abort) would fail — proving network failure is
    // delivered as a `Result` value, fail-soft.
    let file = temp_ql("closed_port", &failure_program(&closed_address()));
    assert_eq!(
        jit_run(&file),
        Some(0),
        "a refused connection must come back as NotOk for the program to match, not crash"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_tcp_request_round_trips_and_forces() {
    let Some(linker) = available_linker() else {
        eprintln!("skipping AOT @tcpRequest gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let (address, server) = spawn_pong_server(2);

    let build = |tag: &str, expected: &str| -> PathBuf {
        let source = temp_ql(tag, &program(&address, expected));
        let binary =
            std::env::temp_dir().join(format!("quilon_tcp_aot_{tag}_{}", std::process::id()));
        let out = Command::new(quilon)
            .args(["build", source.to_str().unwrap(), "--linker", linker])
            .args(["-o", binary.to_str().unwrap()])
            .output()
            .expect("run quilon build");
        assert!(
            out.status.success(),
            "`quilon build` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_file(&source);
        binary
    };

    let match_binary = build("match", "PONG\\n");
    assert_eq!(
        run(Command::new(&match_binary)),
        Some(0),
        "native AOT: @tcpRequest should force to the server's response and pass"
    );

    let mismatch_binary = build("mismatch", "NOPE\\n");
    assert_eq!(
        run(Command::new(&mismatch_binary)),
        Some(101),
        "native AOT: a non-matching expectation must trip the assertion"
    );

    server.join().expect("server thread");
    let _ = std::fs::remove_file(&match_binary);
    let _ = std::fs::remove_file(&mismatch_binary);
}
