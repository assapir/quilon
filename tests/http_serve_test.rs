//! End-to-end proof of `http.@serve`: a GET, a POST, a malformed request line, and a
//! `/quit` path that stops the server through an atomic global — the same pattern
//! `tcp_serve_test.rs` uses for the raw layer underneath, one level up the stack. The
//! client side is a plain `std::net::TcpStream` writing the request bytes by hand and
//! reading the reply to EOF (the server always closes after one response), under both the
//! in-process JIT (`quilon run`) and a native AOT binary (`quilon build`).

mod common;

use common::ensure_runtime_lib;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A port nothing is bound to right now — bind an ephemeral one and drop it immediately,
/// same race every other test in this suite that hands a chosen port to a subprocess
/// accepts (`tcp_serve_test.rs`'s `free_port`).
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to find a free port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// The program under test: the `hummus` handler from the issue's own surface, serving on
/// `address`, with `/quit` (matched on `request.path()`, not a real router — there is none)
/// killing the server through the top-level atomic `server` a handler declared before
/// `@serve` returns must reach this way (the same shape `tcp_serve_test.rs`'s `program`
/// uses for the raw layer).
fn program(address: &str) -> String {
    format!(
        r#"
<< core.http
<< core.net

@server := net.Server {{ handle = 0 }}

killAndReply = () -> http.Response => <
  server.kill(1)
  http.Response.ok("bye")
>

answer = (request :: http.Request) -> http.Response => <
  request.method ?
    | http.Get        => http.Response.ok("chickpeas: plenty")
    | http.Post(body) => http.Response.created("stocked " + body.content)
    | _               => http.Response.status(405)
>

hummus = (request :: http.Request) -> http.Response => <
  request.path() == "/quit" ? killAndReply() : answer(request)
>

^ = () -> Num => <
  server := http.@serve("{address}", request => hummus(request))
  0
>
"#
    )
}

/// Connect to `host:port` with a bounded read/write timeout on every op, mirroring
/// `tcp_serve_test.rs`'s own helper: a bug that leaves a connection open with nothing
/// arriving fails the test in a few seconds instead of hanging the run.
fn connect_with_timeout(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

/// Connect to `host:port`, retrying (bounded) until the server is up — the process under
/// test needs a moment after starting before `@serve` has actually bound and is accepting.
fn connect_once_listening(host: &str, port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match connect_with_timeout(host, port) {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("never managed to connect to the test server: {error}"),
        }
    }
}

/// Send `request` over a fresh connection and read the reply to EOF — valid because the
/// server always answers with `connection: close` and closes right after, one response per
/// connection (this version's own rule). `first` waits out the server's own startup instead
/// of failing the moment it is not there yet.
fn send_raw(host: &str, port: u16, request: &[u8], first: bool) -> String {
    let mut stream = if first {
        connect_once_listening(host, port)
    } else {
        connect_with_timeout(host, port).expect("connect to the running server")
    };
    stream.write_all(request).expect("write the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the reply to EOF");
    response
}

/// Wait for `child` to exit, killing it and failing loudly instead of hanging the test run
/// if it does not within `timeout` (mirrors `tcp_serve_test.rs`'s own helper).
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

/// Drive a GET, a POST, a malformed request line, and finally `/quit` against a server
/// already listening on `host:port`, asserting each reply's status line, `content-length`,
/// and body.
fn drive_get_post_malformed_then_quit(host: &str, port: u16) {
    let get = send_raw(
        host,
        port,
        b"GET /pantry HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
        true,
    );
    assert!(get.starts_with("HTTP/1.1 200 OK\r\n"), "GET reply: {get}");
    assert!(get.contains("content-length: 17\r\n"), "GET reply: {get}");
    assert!(get.ends_with("chickpeas: plenty"), "GET reply: {get}");

    let post = send_raw(
        host,
        port,
        b"POST /pantry HTTP/1.1\r\nHost: shop\r\nContent-Length: 5\r\nConnection: close\r\n\r\nbeans",
        false,
    );
    assert!(
        post.starts_with("HTTP/1.1 201 Created\r\n"),
        "POST reply: {post}"
    );
    assert!(post.contains("content-length: 8\r\n"), "POST reply: {post}");
    // The request body is never read: a POST arrives with an empty body.
    assert!(post.ends_with("stocked "), "POST reply: {post}");

    let malformed = send_raw(host, port, b"GARBAGE\r\n\r\n", false);
    assert!(
        malformed.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "malformed reply: {malformed}"
    );
    assert!(
        malformed.contains("content-length: 0\r\n"),
        "malformed reply: {malformed}"
    );

    // `kill` force-closes every still-open connection once its grace period ends,
    // including — the calling handler's own excluded from the WAIT, not from this — the
    // very connection this request arrived on, racing the reply this handler is about to
    // write; `tcp_serve_test.rs`'s own `quit` connection has the same shape and does not
    // read a reply either. What matters here is only that the request reaches the handler
    // (a well-formed one does) and the process exits once `kill` settles.
    let mut quitter = connect_with_timeout(host, port).expect("connect to send quit");
    quitter
        .write_all(b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the quit request");
    drop(quitter);
}

#[test]
fn jit_http_serve_answers_then_kill_stops_the_server() {
    let port = free_port();
    let file = common::temp_ql("http_serve_jit", &program(&format!("127.0.0.1:{port}")));

    let child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    drive_get_post_malformed_then_quit("127.0.0.1", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_http_serve_answers_then_kill_stops_the_server() {
    let Some(linker) = ["clang", "gcc"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }) else {
        eprintln!("skipping AOT http.@serve gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let port = free_port();
    let source = common::temp_ql("http_serve_aot", &program(&format!("127.0.0.1:{port}")));
    let binary = std::env::temp_dir().join(format!("quilon_http_serve_aot_{}", std::process::id()));
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

    drive_get_post_malformed_then_quit("127.0.0.1", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "native AOT: the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&binary);
}
