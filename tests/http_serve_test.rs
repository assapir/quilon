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
use std::net::TcpStream;
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
  http.Response.reply(http.OK, "bye")
>

answer = (request :: http.Request) -> http.Response => <
  request.method ?
    | http.Get        => http.Response.reply(http.OK, "chickpeas: plenty")
    | http.Head       => http.Response.reply(http.OK, "chickpeas: plenty")
    | http.Post(body) => http.Response.reply(http.Created, "stocked " + body.content)
    | _               => http.Response.reply(http.MethodNotAllowed)
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
    | Ok(reply)    => checkReply(reply, 201, "stocked beans")
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

/// Send `request` over a fresh connection and read the reply to EOF — valid for a request
/// that itself asks to close, or one malformed enough that the server closes regardless
/// (both still end the connection after one response). Connects once and fails fast: by
/// the time any caller reaches this, `wait_until_listening` has already confirmed the
/// server is up, so a connect failure here is a genuine problem, not the server still
/// starting.
fn send_raw(host: &str, port: u16, request: &[u8]) -> String {
    let mut stream = connect_with_timeout(host, port).expect("connect to the running server");
    stream.write_all(request).expect("write the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the reply to EOF");
    response
}

/// The earliest index at which `needle` occurs in `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads HTTP responses one at a time off a persistent connection — the keep-alive tests'
/// own counterpart of `send_raw`, which cannot be used once a connection outlives its
/// first response. Keeps its own read buffer across calls: a server fast enough to answer
/// two pipelined requests before this side's next `read()` call can land both replies in
/// one `TcpStream::read` — a fresh buffer per call would silently drop the second reply's
/// own bytes, and the read for it would then wait on a socket nothing more is coming on.
struct ResponseReader<'a> {
    stream: &'a mut TcpStream,
    buffer: Vec<u8>,
}

impl<'a> ResponseReader<'a> {
    fn new(stream: &'a mut TcpStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
        }
    }

    /// Read exactly one HTTP response: until the head's blank line, then exactly
    /// `Content-Length` more bytes (every reply this suite's own handlers write is a
    /// short, non-chunked body). Anything past that stays in `self.buffer` for the next
    /// call, whether it arrived just now or on an earlier read.
    fn next_response(&mut self) -> String {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(head_end) = find_subslice(&self.buffer, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&self.buffer[..head_end]);
                let content_length: usize = head
                    .split("\r\n")
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                let total_needed = head_end + 4 + content_length;
                if self.buffer.len() >= total_needed {
                    let response =
                        String::from_utf8_lossy(&self.buffer[..total_needed]).into_owned();
                    self.buffer.drain(..total_needed);
                    return response;
                }
            }
            let read = self.stream.read(&mut chunk).expect("read one response");
            assert!(read > 0, "connection closed before a full response arrived");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
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

/// A server whose handler echoes a POST's body back as the reply (`Created` carrying
/// exactly `body.content`), built with the three-argument `http.@serve` so `max_body_size`
/// governs how large a request body it accepts before answering `413` — the same
/// atomic-global `/quit` shape `program` above uses for the raw layer's own tests.
fn body_echo_program(address: &str, max_body_size: u64) -> String {
    format!(
        r#"
<< core.http
<< core.net

@server := net.Server {{ handle = 0 }}

killAndReply = () -> http.Response => <
  server.kill(1)
  http.Response.reply(http.OK, "bye")
>

echo = (request :: http.Request) -> http.Response => <
  request.method ?
    | http.Post(body) => http.Response.reply(http.Created, body.content)
    | _                => http.Response.reply(http.MethodNotAllowed)
>

hummus = (request :: http.Request) -> http.Response => <
  request.path() == "/quit" ? killAndReply() : echo(request)
>

^ = () -> Num => <
  server := http.@serve(
    "{address}", request => hummus(request),
    http.ServerOptions {{ maxBodySize = {max_body_size}, idleTimeout = 5 }})
  0
>
"#
    )
}

/// Spawn `body_echo_program` under the JIT with `max_body_size`, wait for it to start
/// listening, run `drive` against it with raw sockets, then quit and wait for a clean
/// exit — the shape every body-cap/framing test below shares.
fn run_body_echo_server(max_body_size: u64, drive: impl FnOnce(&str, u16)) {
    let port = free_port();
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let file = common::temp_ql(
        "http_serve_body_echo",
        &body_echo_program(&format!("127.0.0.1:{port}"), max_body_size),
    );

    let child = Command::new(quilon)
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    wait_until_listening("127.0.0.1", port);
    drive("127.0.0.1", port);

    let mut quitter = connect_with_timeout("127.0.0.1", port).expect("connect to send quit");
    quitter
        .write_all(b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the quit request");
    drop(quitter);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_http_serve_rejects_a_body_over_the_cap_and_malformed_chunked_framing() {
    run_body_echo_server(10, |host, port| {
        // A declared `Content-Length` past the cap is rejected the moment the head
        // arrives — no need to send the 999 bytes it claims for the check to run.
        let over_cap = send_raw(
            host,
            port,
            b"POST /orders HTTP/1.1\r\nHost: shop\r\nContent-Length: 999\r\nConnection: close\r\n\r\n",
        );
        assert!(
            over_cap.starts_with("HTTP/1.1 413 Content Too Large\r\n"),
            "over-cap reply: {over_cap}"
        );

        let malformed = send_raw(
            host,
            port,
            b"POST /orders HTTP/1.1\r\nHost: shop\r\nTransfer-Encoding: chunked\r\n\
              Connection: close\r\n\r\nZZ\r\nhello\r\n0\r\n\r\n",
        );
        assert!(
            malformed.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "malformed chunked reply: {malformed}"
        );
    });
}

#[test]
fn jit_http_serve_reads_a_content_length_and_a_chunked_body_under_a_raised_cap() {
    run_body_echo_server(16, |host, port| {
        // 15 bytes, one under the 16-byte cap this server raised to — the same body a
        // 10-byte cap (the test above) would have rejected outright.
        let content_length_reply = send_raw(
            host,
            port,
            b"POST /orders HTTP/1.1\r\nHost: shop\r\nContent-Length: 15\r\n\
              Connection: close\r\n\r\nextra falafel!!",
        );
        assert!(
            content_length_reply.starts_with("HTTP/1.1 201 Created\r\n"),
            "content-length reply: {content_length_reply}"
        );
        assert!(
            content_length_reply.ends_with("extra falafel!!"),
            "content-length reply: {content_length_reply}"
        );

        let chunked_reply = send_raw(
            host,
            port,
            b"POST /orders HTTP/1.1\r\nHost: shop\r\nTransfer-Encoding: chunked\r\n\
              Connection: close\r\n\r\n6\r\nhummus\r\n0\r\n\r\n",
        );
        assert!(
            chunked_reply.starts_with("HTTP/1.1 201 Created\r\n"),
            "chunked reply: {chunked_reply}"
        );
        assert!(
            chunked_reply.ends_with("hummus"),
            "chunked reply: {chunked_reply}"
        );
    });
}

/// Spawn `program` (the GET/POST/quit handler, default `ServerOptions`, so a 5-second
/// idle timeout and keep-alive as the connection default) under the JIT, wait for it to
/// start listening, run `drive` against it with raw sockets, quit through `/quit`, and
/// wait for a clean exit — the keep-alive tests' own counterpart of `run_body_echo_server`.
fn run_hummus_server(drive: impl FnOnce(&str, u16)) {
    let port = free_port();
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let file = common::temp_ql(
        "http_serve_keep_alive",
        &program(&format!("127.0.0.1:{port}")),
    );

    let child = Command::new(quilon)
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    wait_until_listening("127.0.0.1", port);
    drive("127.0.0.1", port);

    let mut quitter = connect_with_timeout("127.0.0.1", port).expect("connect to send quit");
    quitter
        .write_all(b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the quit request");
    drop(quitter);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_http_serve_keeps_a_connection_alive_for_a_second_request() {
    run_hummus_server(|host, port| {
        let mut stream = connect_with_timeout(host, port).expect("connect to the server");
        let mut reader = ResponseReader::new(&mut stream);

        reader
            .stream
            .write_all(b"GET /pantry HTTP/1.1\r\nHost: shop\r\n\r\n")
            .expect("write the first request");
        let first = reader.next_response();
        assert!(
            first.starts_with("HTTP/1.1 200 OK\r\n"),
            "first reply: {first}"
        );
        assert!(
            first.contains("connection: keep-alive\r\n"),
            "first reply: {first}"
        );

        // The same connection, a second request: only possible if the server kept it open.
        reader
            .stream
            .write_all(b"GET /pantry HTTP/1.1\r\nHost: shop\r\n\r\n")
            .expect("write the second request");
        let second = reader.next_response();
        assert!(
            second.starts_with("HTTP/1.1 200 OK\r\n"),
            "second reply: {second}"
        );
    });
}

#[test]
fn jit_http_serve_closes_after_a_request_that_asks_for_connection_close() {
    run_hummus_server(|host, port| {
        // `send_raw` reads to EOF: it only returns if the server actually closes after
        // this one reply, despite that reply's own header defaulting to keep-alive — the
        // request's own wish is what ends it, exactly as `shouldCloseAfter` decides.
        let reply = send_raw(
            host,
            port,
            b"GET /pantry HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
        );
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "reply: {reply}");
        assert!(
            reply.contains("connection: keep-alive\r\n"),
            "reply: {reply}"
        );
    });
}

#[test]
fn jit_http_serve_closes_after_one_response_for_an_http10_request() {
    run_hummus_server(|host, port| {
        // No `Connection` header at all: HTTP/1.0's own default is close, not keep-alive,
        // so the request line alone must end the connection after this one reply.
        let reply = send_raw(host, port, b"GET /pantry HTTP/1.0\r\nHost: shop\r\n\r\n");
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "reply: {reply}");
    });
}

#[test]
fn jit_http_serve_head_reply_carries_no_body_but_the_correct_content_length() {
    run_hummus_server(|host, port| {
        // `Connection: close` so `send_raw`'s read-to-EOF proves there is nothing past the
        // blank line at all — a body-bearing reply of the same length would not end there.
        let reply = send_raw(
            host,
            port,
            b"HEAD /pantry HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
        );
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "reply: {reply}");
        // "chickpeas: plenty" is 17 bytes — the same content-length a GET of the same
        // handler would send, even though HEAD's own reply carries none of those bytes.
        assert!(reply.contains("content-length: 17\r\n"), "reply: {reply}");
        assert!(reply.ends_with("\r\n\r\n"), "reply carried a body: {reply}");
    });
}

#[test]
fn jit_http_serve_answers_two_pipelined_requests_in_one_write() {
    run_hummus_server(|host, port| {
        let mut stream = connect_with_timeout(host, port).expect("connect to the server");
        let mut reader = ResponseReader::new(&mut stream);
        let both = [
            &b"GET /pantry HTTP/1.1\r\nHost: shop\r\n\r\n"[..],
            &b"GET /pantry HTTP/1.1\r\nHost: shop\r\n\r\n"[..],
        ]
        .concat();
        // Both requests in ONE write: the second's bytes arrive before the first response
        // is written, so the server must pick them up from `leftoverAfterHead` rather than
        // a fresh read. A server fast enough may likewise answer both before this side
        // reads at all, landing both replies in one `read()` — `ResponseReader`'s own
        // buffer is what keeps the second reply's bytes from being dropped on the floor.
        reader
            .stream
            .write_all(&both)
            .expect("write both requests in one go");

        let first = reader.next_response();
        assert!(
            first.starts_with("HTTP/1.1 200 OK\r\n"),
            "first reply: {first}"
        );
        let second = reader.next_response();
        assert!(
            second.starts_with("HTTP/1.1 200 OK\r\n"),
            "second reply: {second}"
        );
    });
}

/// A server whose only purpose is proving `ServerOptions.idleTimeout`: `/quit` is still
/// there so the test can clean up regardless of what the timeout under test does, but the
/// timeout that matters is `idleTimeout` seconds of silence on the connection under test —
/// kept well under a second so the test itself stays fast.
fn idle_timeout_program(address: &str, idle_timeout: f64) -> String {
    format!(
        r#"
<< core.http
<< core.net

@server := net.Server {{ handle = 0 }}

killAndReply = () -> http.Response => <
  server.kill(1)
  http.Response.reply(http.OK, "bye")
>

hummus = (request :: http.Request) -> http.Response => <
  request.path() == "/quit"
    ? killAndReply()
    : http.Response.reply(http.OK, "chickpeas: plenty")
>

^ = () -> Num => <
  server := http.@serve(
    "{address}", request => hummus(request),
    http.ServerOptions {{ maxBodySize = 16 * 1024 * 1024, idleTimeout = {idle_timeout} }})
  0
>
"#
    )
}

#[test]
fn jit_http_serve_closes_an_idle_connection_after_its_configured_timeout() {
    let port = free_port();
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let file = common::temp_ql(
        "http_serve_idle_timeout",
        &idle_timeout_program(&format!("127.0.0.1:{port}"), 0.2),
    );

    let child = Command::new(quilon)
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run");

    wait_until_listening("127.0.0.1", port);

    let start = Instant::now();
    let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect and go idle");
    // Never writes anything: `idleTimeout` alone, not a peer close, must end this.
    let mut buffer = [0u8; 1];
    let read = stream.read(&mut buffer).expect("read the idle close");
    assert_eq!(read, 0, "the idle connection was closed, not left open");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "the idle timeout should close the connection in well under 2s, took {:?}",
        start.elapsed()
    );

    let mut quitter = connect_with_timeout("127.0.0.1", port).expect("connect to send quit");
    quitter
        .write_all(b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the quit request");
    drop(quitter);

    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );
    let _ = std::fs::remove_file(&file);
}
