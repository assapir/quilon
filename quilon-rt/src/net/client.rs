// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! `net.@tcpRequest`: a one-shot TCP request/response exchange.
//!
//! [`__tcp_request_launch`] wires this tier to a Quilon `@` primitive: it backs the
//! internal `@tcpRequest` request-exchange primitive (connect, write the request, read
//! the response until the peer closes) as a background producer over the generic
//! deferral core, returning a deferred `Result` — `Ok(responseBytes)` on success,
//! `NotOk(errorMessage)` on any network failure. No failure terminates the process; the
//! outcome flows back to `.qn` code to match on. It is internal — the HTTP client sits
//! on it; users do not import raw sockets. Built on [`super::TcpStream`] and
//! `super::resolve`, the plumbing shared with [`super::server`].

use super::{TcpStream, bytes_to_string, copy_bytes, resolve};
use crate::deferred::{QnResult, launch_deferred_result};
use std::io;

/// The most bytes `@tcpRequest` buffers for one response. A close-delimited read has no length
/// header, so without a bound a peer that never closes would grow the buffer until memory ran
/// out. `pub(super)`: [`super::tls`] shares this cap.
pub(super) const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// `@tcpRequest(address, requestBytes)` and its `ConnectOptions` overload: launch a one-shot TCP
/// request exchange on a background fiber, returning the deferred `Result` immediately. `tls`/
/// `unchecked_certificates` (each `0`/`1`, the flattened `Transport`/`Certificates` discriminants
/// — see `generate_at_primitive`'s `"tcpRequest"` arm) pick the plain exchange below or
/// `super::tls::tls_request`; the two-argument call site passes `0, 0`.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QnResult`]; `address_data`/`request_data` are null,
/// or point to `address_len`/`request_len` readable bytes for the duration of this call (a
/// `Text`'s live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_request_launch(
    out: *mut QnResult,
    address_data: *const u8,
    address_len: i64,
    request_data: *const u8,
    request_len: i64,
    tls: i64,
    unchecked_certificates: i64,
) {
    let address = bytes_to_string(address_data, address_len);
    let request = copy_bytes(request_data, request_len);
    let tls = tls != 0;
    let unchecked = unchecked_certificates != 0;
    let deferred = launch_deferred_result(move || {
        if tls {
            match super::tls::tls_request(&address, &request, unchecked) {
                Ok(response) => QnResult::ok(&response),
                Err(message) => QnResult::not_ok(&message),
            }
        } else {
            tcp_request(&address, &request)
        }
    });
    // SAFETY: `out` is writable storage for one `QnResult` (the code generator's alloca).
    unsafe { *out = deferred };
}

/// The `@tcpRequest` producer: perform the whole request exchange against `address` and return
/// its outcome as a `Result`. On success it yields `Ok(responseBytes)`; on ANY failure — address
/// resolution, connect, write, read, or an over-cap response — it yields `NotOk(message)` naming
/// the failing stage and the address. Fail-soft: no failure terminates the process.
fn tcp_request(address: &str, request: &[u8]) -> QnResult {
    let target = match resolve(address) {
        Ok(target) => target,
        Err(error) => return request_error(address, "resolve", &error),
    };
    let mut stream = match TcpStream::connect(target) {
        Ok(stream) => stream,
        Err(error) => return request_error(address, "connect", &error),
    };
    if let Err(error) = stream.write_all(request) {
        return request_error(address, "write", &error);
    }
    match read_to_close(&mut stream) {
        Ok(response) => QnResult::ok(&response),
        Err(error) => request_error(address, "read", &error),
    }
}

/// Read from `reader` until it hits EOF, returning every byte received — the close-delimited
/// read the one-connection-per-request exchange relies on. Generic over `io::Read` so both the
/// plain path (`TcpStream`, whose `Ok(0)` means EOF) and [`super::tls`]'s (a `StreamOwned`, where
/// a peer closing without `close_notify` surfaces as `UnexpectedEof` instead) share it.
pub(super) fn read_to_close<R: io::Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(response),
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > MAX_RESPONSE_BYTES {
                    return Err(io::Error::other(format!(
                        "response exceeded the {MAX_RESPONSE_BYTES}-byte cap"
                    )));
                }
            }
            Err(ref error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(response),
            Err(error) => return Err(error),
        }
    }
}

/// Build the `NotOk(message)` a failed `@tcpRequest` yields: the failing `stage`
/// (`resolve`/`connect`/`write`/`read`), the target `address`, and the underlying error.
fn request_error(address: &str, stage: &str, error: &io::Error) -> QnResult {
    QnResult::not_ok(&request_error_text(address, stage, error))
}

/// [`request_error`]'s message, as plain text — shared with [`super::tls`]'s non-handshake
/// stages (resolve, connect, post-handshake write/read).
pub(super) fn request_error_text(address: &str, stage: &str, error: &io::Error) -> String {
    format!("core.net.@tcpRequest to {address} failed at {stage}: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc_test_harness::on_gc_thread;
    use crate::scheduler::{run, spawn};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn tcp_request_round_trips_against_a_local_listener() {
        // The whole `@tcpRequest` path end to end: the primitive LAUNCHES a background producer
        // that connects to a real listener, writes the request, and reads the response until the
        // peer closes; a separate fiber FORCES the deferred value; the `Ok(responseBytes)` flows
        // back.
        use crate::deferred::{__force_result, RESULT_OK_TAG};
        use crate::mem::QnSlice;

        // A zeroed `Result` out-parameter for the FFI calls to fill.
        let blank = || QnResult {
            tag: 0,
            slot: QnSlice::empty(),
        };

        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static TAG: AtomicUsize = AtomicUsize::new(0);
        GOT.lock().unwrap().clear();
        TAG.store(usize::MAX, Ordering::SeqCst);

        // A blocking std listener on its own OS thread stands in for a peer: accept one
        // connection, read the request, write a fixed response, then close (close-delimited —
        // dropping the stream is what ends the client's read-to-close).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = [0u8; 5];
            conn.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"PING\n");
            conn.write_all(b"PONG\n").unwrap();
        });

        // `*const u8` isn't `Send`, so the pointers cross the closure as `usize`.
        let (address_ptr, address_len) = crate::test_support::text_of(&format!("{addr}"));
        let (request_ptr, request_len) = crate::test_support::text_of("PING\n");
        let (address_ptr, request_ptr) = (address_ptr as usize, request_ptr as usize);
        on_gc_thread(move || {
            run(move || {
                let mut deferred = blank();
                __tcp_request_launch(
                    &mut deferred,
                    address_ptr as *const u8,
                    address_len,
                    request_ptr as *const u8,
                    request_len,
                    0,
                    0,
                );
                let deferred_ptr = deferred.slot.data;
                spawn(move || {
                    let mut forced = blank();
                    __force_result(&mut forced, deferred_ptr);
                    TAG.store(forced.tag as usize, Ordering::SeqCst);
                    let bytes =
                        crate::text::byte_slice(forced.slot.data as *const u8, forced.slot.len);
                    *GOT.lock().unwrap() = bytes.to_vec();
                });
            });
        });

        server.join().unwrap();
        assert_eq!(
            TAG.load(Ordering::SeqCst),
            RESULT_OK_TAG as usize,
            "Ok variant"
        );
        assert_eq!(&*GOT.lock().unwrap(), b"PONG\n");
    }
}
