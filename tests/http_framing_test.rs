//! End-to-end proof of `core.http`'s body framing over a real socket.
//!
//! `Response.body()` and `Request.send()` sit on the native `__http_frame_body` intrinsic
//! (`quilon-rt/src/http.rs`) for dechunking and `Content-Length`. These tests spawn a tiny
//! LOCAL listener on a background thread that answers with a real chunked or
//! `Content-Length` reply, then compile and run a program that `send()`s a request against
//! it — proving the framing runs end to end on both the in-process JIT (`quilon run`) and,
//! when a linker is present, a native AOT binary (`quilon build`). One case's chunk boundary
//! falls inside a multi-byte character — the reason this framing is native rather than
//! Quilon, which slices `Text` by grapheme, not byte.

mod common;

use common::ensure_runtime_lib;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;

/// A program that `GET`s `address` and asserts the reply's `body()` equals `expected` —
/// reaching this arm at all already proves `send()` came back `Ok`.
fn get_program(address: &str, expected: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.http

^ = () -> Num => <
  http.Request {{ method = http.Get, url = "http://{address}/" }}.send() ?
    | Ok(response) => assert(response.body(), equals("{expected}"))
    | NotOk(error) => test.failAt(error)
  0
>
"#
    )
}

/// A program that `GET`s `address` and passes only if `send()` comes back `NotOk` — a
/// malformed reply's framing failure delivered as a value, not a partial body.
fn expect_not_ok_program(address: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.http

^ = () -> Num => <
  http.Request {{ method = http.Get, url = "http://{address}/" }}.send() ?
    | Ok(_)    => test.failAt("expected malformed framing to fail")
    | NotOk(_) => $
  0
>
"#
    )
}

/// A program that sends a `HEAD` to `address` and passes only if `send()` comes back `Ok` —
/// the reply's `Content-Length` describes a body that is never sent, which the framing check
/// must not mistake for a truncated one.
fn head_program(address: &str) -> String {
    format!(
        r#"
<< core.io
<< core.test
<< core.http

^ = () -> Num => <
  http.Request {{ method = http.Head, url = "http://{address}/" }}.send() ?
    | Ok(_)        => $
    | NotOk(error) => test.failAt(error)
  0
>
"#
    )
}

/// A chunked reply whose two chunks split `é` (`\xC3\xA9`) across the boundary: chunk one
/// ends with `h\xC3`, chunk two opens with `\xA9llo` — the whole reason this framing is
/// native, since a byte-level split has no Quilon-reachable position (`Text` slices by
/// grapheme).
fn chunked_reply_splitting_a_multi_byte_character() -> Vec<u8> {
    let mut reply = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    reply.extend_from_slice(b"2\r\nh\xC3\r\n4\r\n\xA9llo\r\n0\r\n\r\n");
    reply
}

/// A reply that claims a `Content-Length` of 10 but sends only 3 bytes before closing.
fn truncated_content_length_reply() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc".to_vec()
}

/// A `HEAD` reply: `Content-Length` names a body the server never writes.
fn headless_content_length_reply() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nContent-Length: 1234\r\n\r\n".to_vec()
}

/// Bind a loopback listener and serve exactly one connection with the fixed `reply` bytes:
/// read (and discard) the request, write `reply`, then close — dropping the stream is what
/// ends the client's close-delimited read. Returns the `host:port` address to dial and the
/// server thread's handle.
fn spawn_one_shot_server(reply: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local listener");
    let address = listener.local_addr().expect("local addr").to_string();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept a connection");
        let mut buffer = [0u8; 4096];
        let _ = conn.read(&mut buffer);
        conn.write_all(&reply).expect("write the reply");
    });
    (address, handle)
}

/// Write `source` to a unique temp `.qn` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_http_framing_{tag}_{}_{}.qn",
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
fn jit_dechunks_a_reply_whose_chunk_boundary_splits_a_multi_byte_character() {
    let (address, server) = spawn_one_shot_server(chunked_reply_splitting_a_multi_byte_character());
    let file = temp_ql("chunked", &get_program(&address, "h\u{e9}llo"));
    assert_eq!(
        jit_run(&file),
        Some(0),
        "a chunk edge inside 'é' must still dechunk to the whole character"
    );
    server.join().expect("server thread");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_a_short_content_length_reply_sends_not_ok() {
    let (address, server) = spawn_one_shot_server(truncated_content_length_reply());
    let file = temp_ql("truncated", &expect_not_ok_program(&address));
    assert_eq!(
        jit_run(&file),
        Some(0),
        "a Content-Length longer than the bytes present must send NotOk, not a partial body"
    );
    server.join().expect("server thread");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_a_head_reply_with_content_length_sends_ok() {
    let (address, server) = spawn_one_shot_server(headless_content_length_reply());
    let file = temp_ql("head", &head_program(&address));
    assert_eq!(
        jit_run(&file),
        Some(0),
        "a HEAD reply's Content-Length describes a body that is never sent, not a truncation"
    );
    server.join().expect("server thread");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_dechunks_a_reply_whose_chunk_boundary_splits_a_multi_byte_character() {
    let Some(linker) = available_linker() else {
        eprintln!("skipping AOT http framing gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };
    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let (address, server) = spawn_one_shot_server(chunked_reply_splitting_a_multi_byte_character());
    let source = temp_ql("aot_chunked", &get_program(&address, "h\u{e9}llo"));
    let binary =
        std::env::temp_dir().join(format!("quilon_http_framing_aot_{}", std::process::id()));
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
    assert_eq!(
        run(Command::new(&binary)),
        Some(0),
        "native AOT: a chunk edge inside 'é' must still dechunk to the whole character"
    );

    server.join().expect("server thread");
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&binary);
}
