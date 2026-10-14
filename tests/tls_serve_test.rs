//! End-to-end proof of `net.@tcpServe`/`http.@serve`'s listener-list overload and its
//! `TLS` listeners — `core.net.Identity`/`Listener`, the compile and startup errors, and a
//! TLS connection's handler running over the same `Connection` a `Plain` one does.
//!
//! Every TLS listener here serves `tests/fixtures/tls/valid.crt`/`valid.key` (a
//! `localhost` leaf, issued by `tests/fixtures/tls/ca.crt` — see that directory's own
//! README for how they were made), reached either as `Files` (the two paths) or `Base64`
//! (the same two files' content, base64'd, so the fixture for `Files` and the fixture for
//! `Base64` are provably the same bytes).

mod common;

use base64::Engine;
use common::{connect_with_timeout, read_announced_port};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

/// The base64 `core.net.Identity::Base64` takes, over `fixture(name)`'s own bytes —
/// one line, matching `base64 -w0 fullchain.pem`'s own output (no line breaks, where the
/// `base64` crate's `STANDARD` engine already leaves none).
fn base64_of_fixture(name: &str) -> String {
    let bytes = std::fs::read(fixture(name)).expect("read the fixture file");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A `rustls::ClientConfig` trusting ONLY `tests/fixtures/tls/ca.crt` — this file's own
/// stand-in for the OS trust store, exactly as `tests/tls_request_test.rs`'s `SSL_CERT_FILE`
/// does for a Quilon client, but for a plain Rust one.
fn client_config_trusting_fixture_ca() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(fixture("ca.crt"))
        .expect("open ca.crt")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse ca.crt")
    {
        roots.add(cert).expect("add the fixture CA");
    }
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// Connect to `127.0.0.1:port` over TLS, trusting the fixture CA, SNI'd as `localhost` —
/// the real peer `valid.crt` is issued for — write `request`, and return whatever comes
/// back until the peer closes.
fn rustls_round_trip(port: u16, request: &[u8]) -> Vec<u8> {
    let config = client_config_trusting_fixture_ca();
    let server_name = ServerName::try_from("localhost").expect("a valid server name");
    let stream = connect_with_timeout("127.0.0.1", port).expect("connect to the TLS listener");
    let conn = ClientConnection::new(config, server_name).expect("build the client connection");
    let mut tls = StreamOwned::new(conn, stream);
    tls.write_all(request).expect("write the request");
    let mut response = Vec::new();
    let _ = tls.read_to_end(&mut response);
    response
}

fn temp_ql(tag: &str, source: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .prefix(&format!("quilon_tls_serve_{tag}_"))
        .suffix(".qn")
        .tempfile()
        .expect("create temp .qn");
    std::fs::write(file.path(), source).expect("write temp .qn");
    file
}

/// `quilon run <file>`, piping stdin from nothing and stdout/stderr as given — the shared
/// shape every test below spawns the server-under-test with.
fn spawn_server(file: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon run")
}

/// Wait for `child` to exit, killing it and failing loudly instead of hanging the test run
/// if it does not within `timeout` — the same helper `tests/tcp_serve_test.rs`/
/// `http_serve_test.rs`/`multi_worker_test.rs` already use, so a server this file's own
/// `quit` request fails to reach is reaped rather than left running indefinitely.
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

/// A `net.@tcpServe` echo server over ONE `TLS` listener, `identity` filling its
/// `core.net.Identity` literal. Bound dual-stack (`[::]:0`, the same reason
/// `tests/tls_request_test.rs`'s own test peer binds that way) so a `localhost`-hostname
/// client finds it whichever of `127.0.0.1`/`::1` its resolver picks, same as a direct
/// `127.0.0.1` connect. Echoes one line back, and stops once a connection sends `quit`.
fn single_tls_listener_program(identity: &str) -> String {
    format!(
        r#"
<< core.net
<< core.io

@server := net.Server {{ handle = 0 }}

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit\r\n" ? server.kill(1) : connection.@write(line)
  $
>

^ = () -> Num => <
  server := net.@tcpServe(
    [net.Listener {{ address = "[::]:0", transport = net.TLS, identity = {identity} }}],
    connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

#[test]
fn a_tls_listener_with_a_files_identity_answers_a_rustls_client() {
    let identity = format!(
        "net.Files({{ certificate = \"{}\", key = \"{}\" }})",
        fixture("valid.crt").display(),
        fixture("valid.key").display()
    );
    let file = temp_ql("files_rustls", &single_tls_listener_program(&identity));
    let mut child = spawn_server(file.path());
    let port = read_announced_port(&mut child);

    let response = rustls_round_trip(port, b"knock knock\r\n");
    assert_eq!(
        &response, b"knock knock\r\n",
        "the TLS listener echoed back"
    );

    rustls_round_trip(port, b"quit\r\n");
    let exit_code = wait_bounded(child, Duration::from_secs(15));
    assert_eq!(exit_code, 0, "the server exited cleanly after kill");
}

#[test]
fn a_tls_listener_with_a_base64_identity_serves_the_same_certificate_as_files() {
    let identity = format!(
        "net.Base64({{ certificate = \"{}\", key = \"{}\" }})",
        base64_of_fixture("valid.crt"),
        base64_of_fixture("valid.key")
    );
    let file = temp_ql("base64_rustls", &single_tls_listener_program(&identity));
    let mut child = spawn_server(file.path());
    let port = read_announced_port(&mut child);

    let response = rustls_round_trip(port, b"based\r\n");
    assert_eq!(
        &response, b"based\r\n",
        "the Base64 identity served the same cert"
    );

    rustls_round_trip(port, b"quit\r\n");
    assert_eq!(wait_bounded(child, Duration::from_secs(15)), 0);
}

#[test]
fn a_tls_listener_answers_the_quilon_client_over_tcp_request() {
    let identity = format!(
        "net.Files({{ certificate = \"{}\", key = \"{}\" }})",
        fixture("valid.crt").display(),
        fixture("valid.key").display()
    );
    let file = temp_ql(
        "files_quilon_client",
        &single_tls_listener_program(&identity),
    );
    let mut child = spawn_server(file.path());
    let port = read_announced_port(&mut child);

    let client_source = format!(
        r#"
<< core.net
<< core.test

^ = () -> Num => <
  net.@tcpRequest(
    "localhost:{port}", "ping\r\n",
    net.ConnectOptions {{ transport = net.TLS, certificates = net.Checked }}
  ) ?
    | Ok(response)  => assert(response, equals("ping\r\n"))
    | NotOk(error)  => test.failAt(error)
  0
>
"#
    );
    let client_file = temp_ql("files_quilon_client_side", &client_source);
    let status = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", client_file.path().to_str().unwrap()])
        .env("SSL_CERT_FILE", fixture("ca.crt"))
        .env_remove("SSL_CERT_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run the Quilon TLS client");
    assert_eq!(
        status.code(),
        Some(0),
        "the PR 487 Quilon TLS client reached the Quilon TLS server"
    );

    rustls_round_trip(port, b"quit\r\n");
    wait_bounded(child, Duration::from_secs(15));
}

#[test]
fn a_plain_and_a_tls_listener_in_one_serve_both_answer() {
    // `/quit` kills the WHOLE server (both listeners) through an atomic global — the same
    // pattern `tests/http_serve_test.rs` uses, since the client cannot reach `shop` any
    // other way once it is a separate process.
    let source = format!(
        r#"
<< core.http
<< core.net
<< core.io

@shop := net.Server {{ handle = 0 }}

killAndReply = () -> http.Response => <
  shop.kill(1)
  http.Response.reply(http.OK, "bye")
>

hello = (request :: http.Request) -> http.Response => <
  request.path() == "/quit" ? killAndReply() : http.Response.reply(http.OK, "hello")
>

^ = () -> $ => <
  shop := http.@serve(
    [
      net.Listener {{ address = "127.0.0.1:0", transport = net.Plain, identity = net.None }},
      net.Listener {{
        address = "127.0.0.1:0", transport = net.TLS,
        identity = net.Files({{ certificate = "{}", key = "{}" }})
      }}
    ],
    request => hello(request))
  addresses = shop.addresses()
  io.print(addresses[0].port)
  io.print(addresses[1].port)
>
"#,
        fixture("valid.crt").display(),
        fixture("valid.key").display()
    );
    let file = temp_ql("mixed_serve", &source);
    let mut child = spawn_server(file.path());

    // Both listeners' own ports, one per `io.print` line, in `addresses()`'s own list
    // order — read together (unlike `read_announced_port`'s single line) since this
    // program prints two.
    let mut reader = BufReader::new(child.stdout.take().expect("stdout piped"));
    let mut plain_line = String::new();
    reader
        .read_line(&mut plain_line)
        .expect("read the Plain listener's own announced port");
    let plain_port: u16 = plain_line.trim().parse().expect("a valid port");
    let mut tls_line = String::new();
    reader
        .read_line(&mut tls_line)
        .expect("read the TLS listener's own announced port");
    let tls_port: u16 = tls_line.trim().parse().expect("a valid port");

    let mut plain = connect_with_timeout("127.0.0.1", plain_port).expect("connect plain");
    plain
        .write_all(b"GET / HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the plain request");
    let mut plain_response = Vec::new();
    plain
        .read_to_end(&mut plain_response)
        .expect("read the plain reply");
    assert!(
        String::from_utf8_lossy(&plain_response).contains("hello"),
        "the Plain listener answered its own request"
    );

    let tls_response = rustls_round_trip(
        tls_port,
        b"GET / HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
    );
    assert!(
        String::from_utf8_lossy(&tls_response).contains("hello"),
        "the TLS listener, in the SAME @serve call, answered its own request"
    );

    // `/quit`, over the plain listener — stops BOTH listeners, since `kill` is one call
    // on the one `Server` the whole list shares.
    let mut quitter = connect_with_timeout("127.0.0.1", plain_port).expect("connect to quit");
    quitter
        .write_all(b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n")
        .expect("write the quit request");
    drop(quitter);

    let exit_code = wait_bounded(child, Duration::from_secs(15));
    assert_eq!(
        exit_code, 0,
        "kill on the one Server stopped both listeners"
    );
}

#[test]
fn a_literal_listener_with_mismatched_transport_and_identity_is_a_compile_error() {
    let source = r#"
<< core.net

mismatched = net.Listener { address = ":443", transport = net.TLS, identity = net.None }

^ = () -> Num => < 0 >
"#;
    let file = temp_ql("qn359", source);
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["check", file.path().to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run quilon check");
    assert_eq!(
        output.status.code(),
        Some(3),
        "a type error exits with the checker's family digit"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("QN363"),
        "the report names QN363:\n{combined}"
    );
}

/// Build a `net.Listener` whose `transport`/`identity` are NOT written as `net.TLS`/
/// `net.None` literals at the construction site — through parameters instead, so the
/// QN363 compile-time check (which only recognizes a literal's own written shape) cannot
/// catch it, and the mismatch surfaces only when `net.@tcpServe` actually starts the
/// listener.
fn runtime_mismatch_program() -> String {
    r#"
<< core.net

mismatched = (transport :: net.Transport, identity :: net.Identity) -> net.Listener => <
  net.Listener { address = "127.0.0.1:0", transport = transport, identity = identity }
>

^ = () -> Num => <
  server = net.@tcpServe([mismatched(net.TLS, net.None)], connection => $)
  0
>
"#
    .to_string()
}

#[test]
fn a_runtime_mismatched_listener_is_fatal_naming_qn512() {
    let file = temp_ql("qn512", &runtime_mismatch_program());
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.path().to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run quilon run");
    assert_eq!(
        output.status.code(),
        Some(5),
        "a runtime failure exits with family 5"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("QN512"),
        "the report names QN512:\n{stderr}"
    );
}

#[test]
fn an_identity_that_fails_to_load_is_fatal_naming_qn513() {
    let source = r#"
<< core.net

^ = () -> Num => <
  server = net.@tcpServe(
    [net.Listener {
      address = "127.0.0.1:0", transport = net.TLS,
      identity = net.Files({ certificate = "/definitely/does/not/exist.pem", key = "/definitely/does/not/exist.key" })
    }],
    connection => $)
  0
>
"#;
    let file = temp_ql("qn513", source);
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.path().to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run quilon run");
    assert_eq!(
        output.status.code(),
        Some(5),
        "a runtime failure exits with family 5"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("QN513"),
        "the report names QN513:\n{stderr}"
    );
}

#[test]
fn a_garbage_handshake_is_dropped_silently_while_the_server_keeps_serving() {
    let identity = format!(
        "net.Files({{ certificate = \"{}\", key = \"{}\" }})",
        fixture("valid.crt").display(),
        fixture("valid.key").display()
    );
    let file = temp_ql("garbage_handshake", &single_tls_listener_program(&identity));
    let mut child = spawn_server(file.path());
    let port = read_announced_port(&mut child);

    // Not a TLS ClientHello at all — the handshake fails, and the connection is closed
    // with no response, the handler never called.
    let mut garbage = connect_with_timeout("127.0.0.1", port).expect("connect with garbage");
    garbage
        .write_all(b"not a client hello, just noise\r\n")
        .expect("write garbage");
    let mut reply = Vec::new();
    let _ = garbage.read_to_end(&mut reply);
    assert!(
        reply.is_empty(),
        "a failed handshake gets no response at all"
    );
    drop(garbage);

    // The server itself is still serving: a REAL handshake right after still works.
    let response = rustls_round_trip(port, b"still here\r\n");
    assert_eq!(
        &response, b"still here\r\n",
        "the server kept serving past the garbage connection"
    );

    rustls_round_trip(port, b"quit\r\n");
    assert_eq!(wait_bounded(child, Duration::from_secs(15)), 0);
}

#[test]
fn a_silent_client_is_dropped_after_the_handshake_timeout_while_the_server_keeps_serving() {
    let source = format!(
        r#"
<< core.http
<< core.net
<< core.io

@shop := net.Server {{ handle = 0 }}

killAndReply = () -> http.Response => <
  shop.kill(1)
  http.Response.reply(http.OK, "bye")
>

hello = (request :: http.Request) -> http.Response => <
  request.path() == "/quit" ? killAndReply() : http.Response.reply(http.OK, "hello")
>

^ = () -> $ => <
  shop := http.@serve(
    [net.Listener {{
      address = "127.0.0.1:0", transport = net.TLS,
      identity = net.Files({{ certificate = "{}", key = "{}" }})
    }}],
    request => hello(request),
    http.ServerOptions {{ maxBodySize = 1024 * 1024, idleTimeout = 0.2 }})
  io.print(shop.address().port)
>
"#,
        fixture("valid.crt").display(),
        fixture("valid.key").display()
    );
    let file = temp_ql("handshake_timeout", &source);
    let mut child = spawn_server(file.path());
    let port = read_announced_port(&mut child);

    // Connect and send nothing at all: `ServerOptions.idleTimeout` (0.2s here) must bound
    // the handshake itself, not just a connection past it, or this holds its worker and
    // fiber until the test's own socket timeout fires instead.
    let mut silent = connect_with_timeout("127.0.0.1", port).expect("connect silently");
    let mut reply = Vec::new();
    let outcome = silent.read_to_end(&mut reply);
    assert!(
        reply.is_empty() && (matches!(outcome, Ok(0)) || outcome.is_err()),
        "a silent client gets no response and is dropped once idleTimeout passes"
    );
    drop(silent);

    // The server itself is still serving: a real handshake right after still works.
    let response = rustls_round_trip(
        port,
        b"GET / HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
    );
    assert!(
        String::from_utf8_lossy(&response).contains("hello"),
        "the server kept serving past the timed-out silent client"
    );

    rustls_round_trip(
        port,
        b"GET /quit HTTP/1.1\r\nHost: shop\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(wait_bounded(child, Duration::from_secs(15)), 0);
}
