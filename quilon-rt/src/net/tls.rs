// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! `net.@tcpRequest`'s `Tls` transport: the same one-shot request/response exchange
//! [`super::client`] performs over plain TCP, but connecting over TLS.
//!
//! The process-wide [`ClientConfig`]s are built once, lazily, the first time either is
//! needed ([`client_config`]) — loading the OS trust store costs milliseconds, so paying
//! it once rather than per connection is the whole point. The handshake itself runs on
//! the runtime's blocking-call pool ([`run_blocking`]), parking only the calling fiber,
//! exactly as DNS resolution does ([`super::resolve_hostname`]); record encryption and
//! decryption run inline on the reactor once the handshake completes — the split matters
//! because a handshake's ECDSA/RSA work can cost single-digit milliseconds, and the
//! reactor thread is shared by every fiber in the process, while a record's AES-GCM cost
//! is close to free.

use super::TcpStream;
use crate::blocking::run_blocking;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError,
    RootCertStore, SignatureScheme,
};
use std::io;
use std::net::{SocketAddr, TcpStream as StdTcpStream};
use std::sync::{Arc, OnceLock};

/// Perform the whole TLS request exchange against `address`: split the host out for
/// SNI/verification, resolve and connect, hand the handshake to the blocking-call pool,
/// then write `request` and read the response until the peer closes — on the reactor,
/// through `stream`'s own park-on-readiness `read`/`write`. Mirrors
/// [`super::client::tcp_request`]'s shape and its `NotOk` text for anything before the
/// handshake; a handshake or record failure is curated by [`curated_message`].
pub(super) fn tls_request(
    address: &str,
    request: &[u8],
    unchecked: bool,
) -> Result<Vec<u8>, String> {
    let host = match host_of(address) {
        Ok(host) => host,
        Err(error) => {
            return Err(super::client::request_error_text(
                address, "resolve", &error,
            ));
        }
    };
    let target = match super::resolve(address) {
        Ok(target) => target,
        Err(error) => {
            return Err(super::client::request_error_text(
                address, "resolve", &error,
            ));
        }
    };
    let server_name = match ServerName::try_from(host.clone()) {
        Ok(name) => name,
        Err(_) => {
            return Err(handshake_error_text(
                address,
                "the host name is not valid for TLS",
            ));
        }
    };

    let owned_host = host.clone();
    let handshake = run_blocking(move || connect_and_handshake(target, server_name, unchecked))
        .unwrap_or_else(|pool_error| Err(HandshakeFailure::Io(pool_error)));
    let (socket, conn) = match handshake {
        Ok(pair) => pair,
        Err(failure) => {
            return Err(handshake_error_text(
                address,
                &curated_message(&failure, &owned_host),
            ));
        }
    };

    if let Err(error) = socket.set_nonblocking(true) {
        return Err(super::client::request_error_text(
            address, "connect", &error,
        ));
    }
    let mut stream = match TcpStream::from_connected(mio::net::TcpStream::from_std(socket)) {
        Ok(stream) => stream,
        Err(error) => {
            return Err(super::client::request_error_text(
                address, "connect", &error,
            ));
        }
    };

    let mut tls = rustls::StreamOwned::new(conn, TcpStreamIo(&mut stream));
    if let Err(error) = io::Write::write_all(&mut tls, request) {
        return Err(record_error_text(address, "write", &error, &owned_host));
    }
    match read_to_close(&mut tls) {
        Ok(response) => Ok(response),
        Err(error) => Err(record_error_text(address, "read", &error, &owned_host)),
    }
}

/// A thin `Read`/`Write` adapter over `&mut TcpStream`, so [`rustls::StreamOwned`] can
/// drive record I/O through the reactor-parked `read`/`write` methods `TcpStream` already
/// has, without `TcpStream` itself needing to implement the standard traits (which would
/// make it, misleadingly, look like an ordinary blocking socket everywhere else it's
/// used).
struct TcpStreamIo<'a>(&'a mut TcpStream);

impl io::Read for TcpStreamIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl io::Write for TcpStreamIo<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Read plaintext from `tls` until the peer closes — [`super::client::read_to_close`]'s
/// counterpart over a TLS stream, with the same response cap.
fn read_to_close(
    tls: &mut rustls::StreamOwned<ClientConnection, TcpStreamIo<'_>>,
) -> io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match io::Read::read(tls, &mut chunk) {
            Ok(0) => return Ok(response),
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > super::client::MAX_RESPONSE_BYTES {
                    return Err(io::Error::other(format!(
                        "response exceeded the {}-byte cap",
                        super::client::MAX_RESPONSE_BYTES
                    )));
                }
            }
            // A close_notify short of a clean shutdown (a peer that just drops the TCP
            // connection) surfaces here rather than as `Ok(0)`; the one-shot exchange
            // treats it the same way the plain path treats EOF.
            Err(ref error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(response),
            Err(error) => return Err(error),
        }
    }
}

/// The blocking-pool job: connect to `target` and drive the TLS handshake to completion,
/// entirely with ordinary blocking I/O on this worker thread — the CPU-bound half of a
/// connection (signature verification, key exchange), kept off the reactor.
fn connect_and_handshake(
    target: SocketAddr,
    server_name: ServerName<'static>,
    unchecked: bool,
) -> Result<(StdTcpStream, ClientConnection), HandshakeFailure> {
    let mut socket = StdTcpStream::connect(target).map_err(HandshakeFailure::Connect)?;
    let mut conn = ClientConnection::new(client_config(unchecked), server_name)
        .map_err(HandshakeFailure::Tls)?;
    conn.complete_io(&mut socket)
        .map_err(HandshakeFailure::Io)?;
    Ok((socket, conn))
}

/// Why [`connect_and_handshake`] didn't produce a connection.
enum HandshakeFailure {
    Connect(io::Error),
    Tls(TlsError),
    Io(io::Error),
}

/// The host portion of a `host:port`/`[ipv6]:port` address — [`ServerName`]'s and the
/// curated error text's own view of who this connection is with, kept apart from the
/// resolved [`SocketAddr`] so a hostname's own text (not its resolved IP) is what SNI and
/// certificate-name checks see.
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
    format!("tls handshake with {address} failed: {reason}")
}

/// A failed post-handshake record read/write: curated the same way a handshake failure
/// is when rustls itself is the cause (a session ticket or post-handshake message can
/// still fail a certificate-adjacent check), else the plain `NotOk` shape the non-TLS
/// stages already use.
fn record_error_text(address: &str, stage: &str, error: &io::Error, host: &str) -> String {
    match downcast_tls_error(error) {
        Some(tls_error) => handshake_error_text(address, &curated_tls_message(tls_error, host)),
        None => super::client::request_error_text(address, stage, error),
    }
}

/// Curate a [`HandshakeFailure`] into the plain-English reason `net.@tcpRequest`'s `NotOk`
/// carries, for the handful of common certificate problems the design calls out by name;
/// anything else falls back to rustls's/`io::Error`'s own text.
fn curated_message(failure: &HandshakeFailure, host: &str) -> String {
    match failure {
        HandshakeFailure::Connect(error) => error.to_string(),
        HandshakeFailure::Tls(error) => curated_tls_message(error, host),
        HandshakeFailure::Io(error) => match downcast_tls_error(error) {
            Some(tls_error) => curated_tls_message(tls_error, host),
            None => error.to_string(),
        },
    }
}

fn downcast_tls_error(error: &io::Error) -> Option<&TlsError> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<TlsError>())
}

fn curated_tls_message(error: &TlsError, host: &str) -> String {
    match error {
        TlsError::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "the certificate is not trusted (issued by an unknown authority)".to_string()
        }
        TlsError::InvalidCertificate(CertificateError::Expired) => {
            "the certificate has expired".to_string()
        }
        TlsError::InvalidCertificate(CertificateError::ExpiredContext { not_after, .. }) => {
            format!("the certificate expired on {}", format_date(*not_after))
        }
        TlsError::InvalidCertificate(CertificateError::NotValidForName) => {
            format!("the certificate is not valid for {host}")
        }
        TlsError::InvalidCertificate(CertificateError::NotValidForNameContext {
            presented,
            ..
        }) => {
            let names: Vec<&str> = presented.iter().map(|name| presented_name(name)).collect();
            format!("the certificate is for {}, not {host}", names.join(", "))
        }
        other => other.to_string(),
    }
}

/// A `NotValidForNameContext::presented` entry as the curated message shows it: rustls-webpki
/// hands each one over as a `Debug`-formatted `GeneralName` (`DnsName("wrong.example.invalid")`,
/// `IpAddress("127.0.0.1")`) — this strips the variant wrapper down to the name itself.
fn presented_name(name: &str) -> &str {
    name.strip_suffix("\")")
        .and_then(|rest| rest.split_once("(\""))
        .map_or(name, |(_, inner)| inner)
}

/// `time` as `YYYY-MM-DD` — the one date a curated error ever formats (a certificate's
/// `notAfter`), so a whole date-handling dependency buys nothing a dozen lines of
/// civil-calendar arithmetic don't already cover.
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

/// The process-wide `ClientConfig`, built once per verification mode the first time it is
/// needed. Two singletons (checked/unchecked), not one parameterized per call — building
/// either loads the OS trust store or installs the accept-any verifier, and neither should
/// repeat per connection.
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

/// The OS trust store via `rustls-native-certs` — which honours `SSL_CERT_FILE`/
/// `SSL_CERT_DIR` itself — falling back to the compiled-in Mozilla list
/// (`webpki-roots`) when the OS store yields no roots at all.
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

/// `Certificates::Unchecked`'s verifier: accepts every certificate chain and name, while
/// still checking the handshake SIGNATURE (that the peer holds the certificate's private
/// key) — skipping trust and name checks, never signature checks, is the standard shape
/// for an intentionally-insecure verifier (rustls's own examples build the same thing).
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
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
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
    ) -> Result<HandshakeSignatureValid, TlsError> {
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
