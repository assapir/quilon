// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! `net.@tcpRequest`'s `TLS` transport.
//!
//! The connect and the handshake's record I/O run on the reactor, exactly like the plain
//! path; only `process_new_packets()` — the CPU-bound step (signature verification, key
//! exchange) — goes to the blocking-call pool ([`run_blocking`]), so a slow or silent peer
//! parks the calling fiber, not a pool thread, and record encryption/decryption afterward
//! runs inline on the reactor.

use super::TcpStream;
use crate::blocking::run_blocking;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, Error as TLSError,
    RootCertStore, SignatureScheme,
};
use std::io;
use std::sync::{Arc, OnceLock};

pub(super) fn tls_request(
    address: &str,
    request: &[u8],
    unchecked: bool,
) -> Result<Vec<u8>, String> {
    let host = host_of(address)
        .map_err(|error| super::client::request_error_text(address, "resolve", &error))?;
    let target = super::resolve(address)
        .map_err(|error| super::client::request_error_text(address, "resolve", &error))?;
    let server_name = ServerName::try_from(host.clone())
        .map_err(|_| handshake_error_text(address, "the host name is not valid for TLS"))?;
    let mut stream = TcpStream::connect(target)
        .map_err(|error| super::client::request_error_text(address, "connect", &error))?;

    let conn = handshake(client_config(unchecked), server_name, &mut stream)
        .map_err(|error| handshake_error_text(address, &curated_message(&error, &host)))?;

    let mut tls = rustls::StreamOwned::new(conn, stream);
    io::Write::write_all(&mut tls, request)
        .map_err(|error| super::client::request_error_text(address, "write", &error))?;
    super::client::read_to_close(&mut tls)
        .map_err(|error| super::client::request_error_text(address, "read", &error))
}

/// Build the `ClientConnection` and drive it to completion. `ClientConnection::new` already
/// does crypto (the ClientHello's key share), so it runs on the blocking-call pool too, not
/// just `process_new_packets` below; `write_tls`/`read_tls` move raw bytes over `stream`'s
/// own park-on-readiness `read`/`write`, the only steps that stay on the reactor.
fn handshake(
    config: Arc<ClientConfig>,
    server_name: ServerName<'static>,
    stream: &mut TcpStream,
) -> io::Result<ClientConnection> {
    let mut conn =
        run_blocking(move || ClientConnection::new(config, server_name))?.map_err(tls_io_error)?;
    while conn.is_handshaking() {
        while conn.wants_write() {
            if conn.write_tls(stream)? == 0 {
                break;
            }
        }
        if !conn.wants_read() {
            continue;
        }
        if conn.read_tls(stream)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed during the TLS handshake",
            ));
        }
        let (returned, outcome) = run_blocking(move || {
            let outcome = conn.process_new_packets();
            (conn, outcome)
        })?;
        conn = returned;
        outcome.map_err(tls_io_error)?;
    }
    Ok(conn)
}

fn tls_io_error(error: TLSError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// The host portion of a `host:port`/`[ipv6]:port` address — kept apart from the resolved
/// `SocketAddr` so SNI and certificate-name checks see the hostname, never a resolved IP.
fn host_of(address: &str) -> io::Result<String> {
    if let Some(rest) = address.strip_prefix('[')
        && let Some((host, _)) = rest.split_once(']')
    {
        return Ok(host.to_string());
    }
    address
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "address has no port"))
}

fn handshake_error_text(address: &str, reason: &str) -> String {
    format!("TLS handshake with {address} failed: {reason}")
}

/// Curate a handshake `io::Error` into the plain-English reason for the common certificate
/// problems; anything else falls back to rustls's/`io::Error`'s own text.
fn curated_message(error: &io::Error, host: &str) -> String {
    match downcast_tls_error(error) {
        Some(tls_error) => curated_tls_message(tls_error, host),
        None => error.to_string(),
    }
}

fn downcast_tls_error(error: &io::Error) -> Option<&TLSError> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<TLSError>())
}

fn curated_tls_message(error: &TLSError, host: &str) -> String {
    match error {
        TLSError::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "the certificate is not trusted (issued by an unknown authority)".to_string()
        }
        TLSError::InvalidCertificate(CertificateError::Expired) => {
            "the certificate has expired".to_string()
        }
        TLSError::InvalidCertificate(CertificateError::ExpiredContext { not_after, .. }) => {
            format!("the certificate expired on {}", format_date(*not_after))
        }
        TLSError::InvalidCertificate(CertificateError::NotValidForName) => {
            format!("the certificate is not valid for {host}")
        }
        TLSError::InvalidCertificate(CertificateError::NotValidForNameContext {
            presented,
            ..
        }) => {
            let names: Vec<&str> = presented.iter().map(|name| presented_name(name)).collect();
            format!("the certificate is for {}, not {host}", names.join(", "))
        }
        other => other.to_string(),
    }
}

/// rustls-webpki hands `NotValidForNameContext::presented` over `Debug`-formatted
/// (`DnsName("wrong.example.invalid")`); this strips the wrapper down to the name itself.
fn presented_name(name: &str) -> &str {
    name.strip_suffix("\")")
        .and_then(|rest| rest.split_once("(\""))
        .map_or(name, |(_, inner)| inner)
}

/// `time` as `YYYY-MM-DD` — the one date a curated error ever formats, so a date-handling
/// dependency buys nothing a dozen lines of civil-calendar arithmetic don't already cover.
fn format_date(time: UnixTime) -> String {
    let (year, month, day) = civil_from_days((time.as_secs() / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since the Unix epoch to a proleptic-Gregorian `(year, month, day)` — Howard
/// Hinnant's `civil_from_days`, public-domain calendar arithmetic
/// (<https://howardhinnant.github.io/date_algorithms.html>).
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// One `ClientConfig` per verification mode, built the first time either is needed —
/// loading the OS trust store costs milliseconds, so once per process rather than once
/// per connection.
fn client_config(unchecked: bool) -> Arc<ClientConfig> {
    static CHECKED: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static UNCHECKED: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let cell = if unchecked { &UNCHECKED } else { &CHECKED };
    Arc::clone(cell.get_or_init(|| Arc::new(build_client_config(unchecked))))
}

fn build_client_config(unchecked: bool) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("ring's cipher suites support rustls's default protocol versions");
    if unchecked {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate { provider }))
            .with_no_client_auth()
    } else {
        builder
            .with_root_certificates(root_store())
            .with_no_client_auth()
    }
}

/// The OS trust store via `rustls-native-certs` (honours `SSL_CERT_FILE`/`SSL_CERT_DIR`),
/// falling back to the compiled-in Mozilla list (`webpki-roots`) when it yields none.
fn root_store() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    roots
}

/// `Certificates::Unchecked`'s verifier: skips trust and name checks but still verifies the
/// handshake signature, the standard shape for an intentionally-insecure verifier.
#[derive(Debug)]
struct AcceptAnyCertificate {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TLSError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TLSError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TLSError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
