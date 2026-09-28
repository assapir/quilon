//! End-to-end proof of `net.@tcpRequest`'s `Tls` transport overload and `core.http`'s
//! `https://`/forced-`Tls` sending, against a REAL TLS peer — a `rustls::ServerConnection`
//! on a background thread, serving the committed fixture certificates under
//! `tests/fixtures/tls/` (a test CA, a `localhost` leaf, an expired leaf, and a
//! wrong-host leaf; see that directory's own certs for how they were made).
//!
//! Trusting the fixture CA is per-subprocess (`SSL_CERT_FILE` set on the spawned `quilon
//! run` `Command`), never process-wide — this test binary's own tests run in parallel, and
//! a process-wide `SSL_CERT_FILE` would leak into every one of them, trusted CA or not.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

fn load_certs(file: &str) -> Vec<CertificateDer<'static>> {
    let bytes = std::fs::read(fixture(file)).expect("read fixture cert");
    rustls_pemfile::certs(&mut bytes.as_slice())
        .collect::<Result<_, _>>()
        .expect("parse fixture cert PEM")
}

fn load_key(file: &str) -> PrivateKeyDer<'static> {
    let bytes = std::fs::read(fixture(file)).expect("read fixture key");
    rustls_pemfile::private_key(&mut bytes.as_slice())
        .expect("parse fixture key PEM")
        .expect("fixture key file carries a key")
}

/// A `ServerConfig` serving `<name>.crt`/`<name>.key` under `tests/fixtures/tls/`.
fn server_config(name: &str) -> Arc<ServerConfig> {
    let certs = load_certs(&format!("{name}.crt"));
    let key = load_key(&format!("{name}.key"));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports rustls's default protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("the fixture cert and key match");
    Arc::new(config)
}

/// Bind a dual-stack loopback listener (so a `localhost`-hostname connect finds it
/// whichever of `127.0.0.1`/`::1` the resolver picks — the same reason the plain-TCP
/// hostname test in `tests/tcp_request_test.rs` binds `[::]:0`), accept exactly one TLS
/// connection under `config`, read whatever the client sends, write `response`, and close.
/// Returns the bound port and the server thread's handle.
///
/// A curated-error test expects the CLIENT to reject the certificate and abort the
/// handshake with a fatal alert, which fails this server's own read/write the same way —
/// expected there, not a bug, so neither is asserted; only the success-path tests (whose
/// client-side assertions already pin the exchange down) rely on `response` actually
/// arriving.
fn spawn_tls_server(config: Arc<ServerConfig>, response: &'static [u8]) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("[::]:0").expect("bind a dual-stack loopback listener");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept a connection");
        let conn = ServerConnection::new(config).expect("build the server connection");
        let mut tls = StreamOwned::new(conn, stream);
        let mut buffer = [0u8; 256];
        // Read the request before responding, so a client that closes right after reading
        // the reply sends no reset the server would otherwise trip over.
        let _ = tls.read(&mut buffer);
        let _ = tls.write_all(response);
    });
    (port, handle)
}

fn temp_ql(tag: &str, source: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .prefix(&format!("quilon_tls_{tag}_"))
        .suffix(".qn")
        .tempfile()
        .expect("create temp .qn");
    std::fs::write(file.path(), source).expect("write temp .qn");
    file
}

/// `quilon run <file>`, with `SSL_CERT_FILE` set to the fixture CA when `trust_ca` — never
/// process-wide, only on this one subprocess.
fn jit_run(file: &std::path::Path, trust_ca: bool) -> Option<i32> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command
        .args(["run", file.to_str().unwrap()])
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if trust_ca {
        command.env("SSL_CERT_FILE", fixture("ca.crt"));
    }
    command.status().expect("run subprocess").code()
}

/// A `net.@tcpRequest` program over TLS against `127.0.0.1:<port>`, asserting the `Ok`
/// response equals `expected`.
fn tls_program(port: u16, certificates: &str, expected: &str) -> String {
    format!(
        r#"
<< core.test
<< core.net

^ = () -> Num => <
  net.@tcpRequest(
    "localhost:{port}", "PING\n",
    net.ConnectOptions {{ transport = net.Tls, certificates = {certificates} }}
  ) ?
    | Ok(response) => assert(response, equals("{expected}"))
    | NotOk(error) => test.failAt(error)
  0
>
"#
    )
}

/// A `net.@tcpRequest` program over TLS expected to fail, asserting the `NotOk` message
/// CONTAINS `expected_reason` — the curated text, not merely "some failure occurred".
fn tls_failure_program(port: u16, certificates: &str, expected_reason: &str) -> String {
    format!(
        r#"
<< core.test
<< core.net

^ = () -> Num => <
  net.@tcpRequest(
    "localhost:{port}", "PING\n",
    net.ConnectOptions {{ transport = net.Tls, certificates = {certificates} }}
  ) ?
    | Ok(_)        => test.failAt("expected a TLS failure")
    | NotOk(error) => assert(error, contains("{expected_reason}"))
  0
>
"#
    )
}

/// An `http.Request.get(url).send()` program asserting the reply's body equals `expected`.
fn https_program(url: &str, expected_body: &str) -> String {
    format!(
        r#"
<< core.test
<< core.http

^ = () -> Num => <
  http.Request.get("{url}").send() ?
    | Ok(response) => assert(response.body(), equals("{expected_body}"))
    | NotOk(error) => test.failAt(error)
  0
>
"#
    )
}

/// Like [`https_program`], but with `RequestOptions.transport = Tls` forcing TLS on a
/// URL that names no scheme at all.
fn forced_tls_program(scheme_less_url: &str, expected_body: &str) -> String {
    format!(
        r#"
<< core.test
<< core.http
<< core.net

^ = () -> Num => <
  options = http.RequestOptions {{
    headers = http.Headers.empty(), transport = net.Tls, certificates = net.Checked
  }}
  http.Request.get("{scheme_less_url}", options).send() ?
    | Ok(response) => assert(response.body(), equals("{expected_body}"))
    | NotOk(error) => test.failAt(error)
  0
>
"#
    )
}

const HTTP_OK_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi";

#[test]
fn tls_request_succeeds_against_a_trusted_certificate() {
    let (port, server) = spawn_tls_server(server_config("valid"), b"PONG\n");
    let file = temp_ql(
        "checked_success",
        &tls_program(port, "net.Checked", "PONG\\n"),
    );
    assert_eq!(
        jit_run(file.path(), true),
        Some(0),
        "a Checked request against a CA-trusted certificate should succeed"
    );
    server.join().expect("server thread");
}

#[test]
fn tls_request_with_unchecked_certificates_ignores_an_untrusted_certificate() {
    // No SSL_CERT_FILE: the test CA is untrusted, and Unchecked accepts the connection
    // anyway.
    let (port, server) = spawn_tls_server(server_config("valid"), b"PONG\n");
    let file = temp_ql(
        "unchecked_success",
        &tls_program(port, "net.Unchecked", "PONG\\n"),
    );
    assert_eq!(
        jit_run(file.path(), false),
        Some(0),
        "Unchecked should accept a certificate the trust store does not"
    );
    server.join().expect("server thread");
}

#[test]
fn tls_request_reports_an_unknown_issuer() {
    let (port, server) = spawn_tls_server(server_config("valid"), b"PONG\n");
    let file = temp_ql(
        "unknown_issuer",
        &tls_failure_program(
            port,
            "net.Checked",
            "the certificate is not trusted (issued by an unknown authority)",
        ),
    );
    assert_eq!(
        jit_run(file.path(), false),
        Some(0),
        "an untrusted certificate should yield the curated unknown-issuer reason"
    );
    server.join().expect("server thread");
}

#[test]
fn tls_request_reports_an_expired_certificate() {
    let (port, server) = spawn_tls_server(server_config("expired"), b"PONG\n");
    let file = temp_ql(
        "expired",
        &tls_failure_program(port, "net.Checked", "the certificate expired on 2020-02-01"),
    );
    assert_eq!(
        jit_run(file.path(), true),
        Some(0),
        "an expired certificate should yield the curated expiry date"
    );
    server.join().expect("server thread");
}

#[test]
fn tls_request_reports_a_wrong_host_certificate() {
    let (port, server) = spawn_tls_server(server_config("wrong_host"), b"PONG\n");
    let file = temp_ql(
        "wrong_host",
        &tls_failure_program(
            port,
            "net.Checked",
            "the certificate is for wrong.example.invalid, not localhost",
        ),
    );
    assert_eq!(
        jit_run(file.path(), true),
        Some(0),
        "a certificate for a different name should name both names in the curated reason"
    );
    server.join().expect("server thread");
}

#[test]
fn https_url_sends_over_tls() {
    let (port, server) = spawn_tls_server(server_config("valid"), HTTP_OK_RESPONSE);
    let url = format!("https://localhost:{port}/");
    let file = temp_ql("https_success", &https_program(&url, "hi"));
    assert_eq!(
        jit_run(file.path(), true),
        Some(0),
        "an https:// URL should connect over TLS and read the reply"
    );
    server.join().expect("server thread");
}

#[test]
fn forced_tls_transport_sends_a_scheme_less_url_over_tls() {
    let (port, server) = spawn_tls_server(server_config("valid"), HTTP_OK_RESPONSE);
    let url = format!("localhost:{port}/");
    let file = temp_ql("forced_tls", &forced_tls_program(&url, "hi"));
    assert_eq!(
        jit_run(file.path(), true),
        Some(0),
        "RequestOptions.transport = Tls should force TLS even for a scheme-less URL"
    );
    server.join().expect("server thread");
}
