//! End-to-end proof of the internal `@tcpRequest` request-exchange socket primitive.
//!
//! `@tcpRequest(address, requestBytes)` launches a background TCP exchange — connect, write the
//! request, read the response until the peer closes — and returns a DEFERRED `Text`; the deferred
//! value flows through a binding and is FORCED where a strict primitive reads its bytes (the
//! comparison inside `assertEq`). These tests spawn a tiny LOCAL listener on a background thread,
//! then compile and run a program that does one `@tcpRequest` against it — proving the response
//! bytes flowed back and forced correctly on both the in-process JIT (`quilon run`) and, when a
//! linker is present, a native AOT binary (`quilon build`).

mod common;

use common::ensure_runtime_lib;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;

/// A program that sends `PING\n` to `address` and asserts the response equals `expected`. The
/// deferred `@tcpRequest` value is forced at the `assertEq` comparison: a matching response exits
/// 0, anything else trips the assertion (exit 101) — which is what proves the REAL response bytes
/// (not a constant) reached the compare.
fn program(address: &str, expected: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.net

^ = () -> Num => <
  response = @tcpRequest("{address}", "PING\n")
  assertEq(response, "{expected}")
  0
>
"#
    )
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

/// Write `source` to a unique temp `.ql` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_tcp_{tag}_{}_{}.ql",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, source).expect("write temp .ql");
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
