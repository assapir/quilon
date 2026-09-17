//! End-to-end proof of `http.@serve`: a GET and a POST driven by `core.http`'s own client
//! (a second, short-lived `quilon run` program), then a malformed request line and a
//! `/quit` path that stops the server through an atomic global driven raw (the client
//! cannot produce a malformed request line at all, and the `/quit` connection's own reply
//! races `kill` — see `drive_malformed_then_quit`) — the same pattern `tcp_serve_test.rs`
//! uses for the raw layer underneath, one level up the stack. Both the client program and
//! the raw checks run under the in-process JIT (`quilon run`) and a native AOT binary
//! (`quilon build`) of the SERVER; the client program itself always runs under the JIT —
//! it is a checking tool here, not the thing under test.

mod common;

use common::{connect_once_listening, connect_with_timeout, ensure_runtime_lib, free_port};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The program under test: the `hummus` handler from `core.http`'s own reference, serving on
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
    | _               => http.Response.status(http.MethodNotAllowed)
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

/// A second, short-lived program: places one GET and one POST order against the server
/// already listening on `address` (`host:port`) with `core.http`'s own client
/// (`http.Request.get`/`post(...).send()`), asserting each reply's status and body — the
/// same checks `examples/http_server.qn` makes of itself. Exits 0 on success; a mismatch or
/// a transport failure trips an assertion (exit 5), which `run_client_check` surfaces.
fn client_check_program(address: &str) -> String {
    format!(
        r#"
<< core.http
<< core.test

checkReply = (reply :: http.Response, expectedStatus :: Num, expectedBody :: Text) -> $ => <
  assert(reply.status().code(), equals(expectedStatus))
  assert(reply.body(), equals(expectedBody))
>

^ = () -> Num => <
  http.Request.get("http://{address}/pantry").send() ?
    | Ok(reply)    => checkReply(reply, 200, "chickpeas: plenty")
    | NotOk(error) => test.failAt(error)

  http.Request.post(
    "http://{address}/pantry", http.Body {{ content = "beans", contentType = "text/plain" }}
  ).send() ?
    | Ok(reply)    => checkReply(reply, 201, "stocked ")
    | NotOk(error) => test.failAt(error)

  0
>
"#
    )
}

/// Run `client_check_program(address)` under the JIT and return its exit code and captured
/// stderr — a diagnostic's own text is worth showing on failure, since a transport failure
/// here (the client cannot reach the server that is, by this point in each test, already
/// listening) is a bug in one of the two layers, not something to work around.
fn run_client_check(quilon: &str, address: &str) -> (Option<i32>, String) {
    let file = common::temp_ql("http_serve_client_check", &client_check_program(address));
    let output = Command::new(quilon)
        .args(["run", file.to_str().unwrap()])
        .output()
        .expect("run the client-check program");
    let _ = std::fs::remove_file(&file);
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Block until the server at `host:port` is accepting connections — the one place any test
/// below waits out the server's own startup, so every later connect attempt (raw or through
/// the client-check program, which retries nothing on its own) can fail fast on a genuine
/// problem instead of a race with `@serve` still binding.
fn wait_until_listening(host: &str, port: u16) {
    drop(connect_once_listening(host, port));
}

/// Send `request` over a fresh connection and read the reply to EOF — valid because the
/// server always answers with `connection: close` and closes right after, one response per
/// connection (this version's own rule). Connects once and fails fast: by the time any
/// caller reaches this, `wait_until_listening` has already confirmed the server is up, so a
/// connect failure here is a genuine problem, not the server still starting.
fn send_raw(host: &str, port: u16, request: &[u8]) -> String {
    let mut stream = connect_with_timeout(host, port).expect("connect to the running server");
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

/// Drive a malformed request line and finally `/quit` against a server already listening
/// on `host:port`, raw — the GET and POST cases are the client program's job
/// (`run_client_check`), since the client cannot produce a malformed request line at all,
/// and the `/quit` reply races `kill` (see the comment below), which a real client would
/// just read as a transport failure rather than the thing this test wants to prove.
fn drive_malformed_then_quit(host: &str, port: u16) {
    let malformed = send_raw(host, port, b"GARBAGE\r\n\r\n");
    assert!(
        malformed.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "malformed reply: {malformed}"
    );
    assert!(
        malformed.contains("content-length: 0\r\n"),
        "malformed reply: {malformed}"
    );

    // A request that is nothing but the blank line is a COMPLETE (if empty) head, not a
    // peer that closed early — it must still get a 400, not silence.
    let blank_only = send_raw(host, port, b"\r\n\r\n");
    assert!(
        blank_only.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "blank-only reply: {blank_only}"
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
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let file = common::temp_ql("http_serve_jit", &program(&format!("127.0.0.1:{port}")));

    let child = Command::new(quilon)
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    wait_until_listening("127.0.0.1", port);
    let (code, stderr) = run_client_check(quilon, &format!("127.0.0.1:{port}"));
    assert_eq!(
        code,
        Some(0),
        "the client-check program must exit 0 (a mismatch or a transport failure trips \
         an assertion in it):\n{stderr}"
    );

    drive_malformed_then_quit("127.0.0.1", port);

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

    wait_until_listening("127.0.0.1", port);
    let (code, stderr) = run_client_check(quilon, &format!("127.0.0.1:{port}"));
    assert_eq!(
        code,
        Some(0),
        "the client-check program must exit 0 (a mismatch or a transport failure trips \
         an assertion in it):\n{stderr}"
    );

    drive_malformed_then_quit("127.0.0.1", port);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "native AOT: the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&binary);
}
