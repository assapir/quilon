// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Non-blocking TCP for the fiber scheduler.
//!
//! [`TcpListener`] and [`TcpStream`] wrap `mio`'s non-blocking sockets and register
//! them with the reactor's `Poll`. Every op that would block parks the calling fiber
//! (via [`crate::scheduler::park_on_readiness`]) instead of spinning or blocking the OS
//! thread: it (re)registers the source for the readiness it needs, yields to the
//! scheduler, and is resumed only when the reactor reports that token ready — exactly
//! the way [`crate::scheduler::sleep`] parks on a deadline. Many sockets thus make
//! progress cooperatively on one thread.
//!
//! [`__tcp_request_launch`] wires this tier to a Quilon `@` primitive: it backs the internal
//! `@tcpRequest` request-exchange primitive (connect, write the request, read the response until
//! the peer closes) as a background producer over the generic deferral core, returning a deferred
//! `Result` — `Ok(responseBytes)` on success, `NotOk(errorMessage)` on any network failure. No
//! failure terminates the process; the outcome flows back to `.ql` code to match on. It is
//! internal — the HTTP client sits on it; users do not import raw sockets.
//!
//! GC note: parking is transparent to the collector. A parked fiber's stack — with
//! its live roots — is scanned by [`crate::gc`]'s `GC_push_other_roots` callback,
//! which pushes every registered fiber that is not currently running, regardless of
//! *why* it is parked. A socket-blocked fiber is therefore covered identically to a
//! sleeping one; `tests::socket_parked_fiber_roots_survive_collection` proves it.

use crate::deferred::{QlResult, launch_deferred_result};
use crate::scheduler::{
    deregister_readiness, park_on_readiness, register_readiness, reregister_readiness,
};
use mio::event::Source;
use mio::{Interest, Token};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};

fn would_block(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

/// Retry a non-blocking op on `source`, parking the fiber on each `WouldBlock` until
/// the reactor reports `interest` ready. Reregistering before every park is what
/// makes the edge-triggered poll re-check readiness and lose no wakeup.
fn io_loop<S: Source, T>(
    source: &mut S,
    token: Token,
    interest: Interest,
    mut op: impl FnMut(&mut S) -> io::Result<T>,
) -> io::Result<T> {
    loop {
        match op(source) {
            Ok(value) => return Ok(value),
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(ref e) if would_block(e) => {
                reregister_readiness(source, token, interest)?;
                park_on_readiness(token);
            }
            Err(e) => return Err(e),
        }
    }
}

/// A non-blocking, reactor-registered TCP listener.
pub struct TcpListener {
    inner: mio::net::TcpListener,
    token: mio::Token,
}

impl TcpListener {
    /// Bind and register for read (connection) readiness.
    pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
        let mut inner = mio::net::TcpListener::bind(addr)?;
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpListener { inner, token })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Accept one connection, parking until a client is ready. The accepted stream is
    /// registered with the reactor for later read/write parking.
    pub fn accept(&mut self) -> io::Result<TcpStream> {
        let (mut inner, _peer) = io_loop(&mut self.inner, self.token, Interest::READABLE, |l| {
            l.accept()
        })?;
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpStream { inner, token })
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

/// A non-blocking, reactor-registered TCP stream.
pub struct TcpStream {
    inner: mio::net::TcpStream,
    token: mio::Token,
}

impl TcpStream {
    /// Initiate a connection and park until it completes. A non-blocking connect
    /// returns immediately; the socket becomes writable once the handshake finishes,
    /// so we register for write readiness, park, then confirm via `SO_ERROR` /
    /// `peer_addr` (a spurious writable wakeup before completion re-parks, never
    /// spins).
    pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        let mut inner = mio::net::TcpStream::connect(addr)?;
        let token = register_readiness(&mut inner, Interest::WRITABLE)?;
        let mut stream = TcpStream { inner, token };
        loop {
            park_on_readiness(stream.token);
            if let Some(err) = stream.inner.take_error()? {
                return Err(err);
            }
            match stream.inner.peer_addr() {
                Ok(_) => return Ok(stream),
                // Handshake not finished yet: re-arm write interest and park again.
                Err(ref e) if e.kind() == io::ErrorKind::NotConnected || would_block(e) => {
                    reregister_readiness(&mut stream.inner, stream.token, Interest::WRITABLE)?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Read once, parking until readable. Returns `Ok(0)` at EOF (peer closed);
    /// connection-reset and other errors propagate. Callers wanting a fixed length
    /// loop over this (partial reads are normal for TCP).
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io_loop(&mut self.inner, self.token, Interest::READABLE, |s| {
            s.read(&mut *buf)
        })
    }

    /// Write once, parking until writable. May write fewer bytes than offered; use
    /// [`write_all`](Self::write_all) to send an entire buffer.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io_loop(&mut self.inner, self.token, Interest::WRITABLE, |s| {
            s.write(buf)
        })
    }

    /// Write the whole buffer, looping over partial writes and parking on each
    /// `WouldBlock`.
    pub fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "wrote zero bytes to socket",
                    ));
                }
                Ok(n) => buf = &buf[n..],
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

/// The most bytes `@tcpRequest` buffers for one response. A close-delimited read has no length
/// header, so without a bound a peer that never closes (or streams without end) would grow the
/// buffer until memory ran out; past this cap the read fails and the exchange yields `NotOk`
/// rather than exhausting memory. 16 MiB comfortably holds an HTTP response the one-shot client
/// is meant for.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// `@tcpRequest(address, requestBytes)`: launch a one-shot TCP request exchange on a background
/// fiber and return the deferred `Result` immediately (the calling fiber does not park here). A
/// THIN wrapper over the generic deferral core, with a socket request-exchange producer: connect
/// to `address`, write the request bytes, read the response until the peer closes (close-delimited
/// — the model the one-connection-per-request HTTP client uses), and hand back all the response
/// bytes. The result is a deferred `Result` — `Ok(responseBytes)` on success, `NotOk(message)` on
/// any failure; the code generator forces it where a strict use reads it.
///
/// The address and request bytes are copied into owned buffers here, before the producer is
/// spawned, so the producer fiber owns its inputs and never reads a `Text` that a later
/// collection might reclaim.
///
/// # Safety contract (upheld by the compiler)
/// `address_data`/`request_data` are null, or point to `address_len`/`request_len` readable
/// bytes for the duration of this call (a `Text`'s live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_request_launch(
    address_data: *const u8,
    address_len: i64,
    request_data: *const u8,
    request_len: i64,
) -> QlResult {
    let address = bytes_to_string(address_data, address_len);
    let request = copy_bytes(request_data, request_len);
    launch_deferred_result(move || tcp_request(&address, &request))
}

/// The `@tcpRequest` producer: perform the whole request exchange against `address` and return
/// its outcome as a `Result`. On success it yields `Ok(responseBytes)`; on ANY failure — address
/// resolution, connect, write, read, or an over-cap response — it yields `NotOk(message)` naming
/// the failing stage and the address. Fail-soft: no failure terminates the process.
fn tcp_request(address: &str, request: &[u8]) -> QlResult {
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
        Ok(response) => QlResult::ok(&response),
        Err(error) => request_error(address, "read", &error),
    }
}

/// Resolve `address` (`host:port`, e.g. `127.0.0.1:8080` or `example.com:80`) to a single
/// [`SocketAddr`], erroring if it names nothing. Note: [`ToSocketAddrs`] does a BLOCKING DNS
/// lookup for a hostname on this cooperative fiber thread; a numeric address (what the local
/// round-trip uses) parses without any network call. Non-blocking DNS is a later refinement — it
/// needs a resolver that can run off the reactor thread, so a slow lookup still stalls the
/// scheduler for now.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "address resolved to no endpoint"))
}

/// Read from `stream` until the peer closes the connection, returning every byte received — the
/// close-delimited read the one-connection-per-request exchange relies on: each partial read
/// parks the fiber on socket readiness, and `Ok(0)` (EOF) ends the loop. Errors once the response
/// would exceed [`MAX_RESPONSE_BYTES`], so an unbounded peer cannot exhaust memory.
fn read_to_close(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(response),
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > MAX_RESPONSE_BYTES {
                    return Err(io::Error::other(format!(
                        "response exceeded the {MAX_RESPONSE_BYTES}-byte cap"
                    )));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// Build the `NotOk(message)` a failed `@tcpRequest` yields: the failing `stage`
/// (`resolve`/`connect`/`write`/`read`), the target `address`, and the underlying error.
fn request_error(address: &str, stage: &str, error: &io::Error) -> QlResult {
    QlResult::not_ok(&format!("@tcpRequest to {address} failed at {stage}: {error}"))
}

/// Copy `len` bytes at `data` into an owned `Vec` (empty if null/empty), so the producer fiber
/// owns its input independent of the caller's `Text`.
///
/// # Safety contract (upheld by the compiler)
/// `data` is null, or points to `len` readable bytes for the duration of this call.
fn copy_bytes(data: *const u8, len: i64) -> Vec<u8> {
    if data.is_null() || len <= 0 {
        return Vec::new();
    }
    // SAFETY: the compiler's contract: `len` readable bytes at `data`, valid for this call.
    unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec()
}

/// Copy `len` bytes at `data` into an owned `String` (empty if null/empty). Invalid UTF-8 is
/// replaced; an address is always valid UTF-8 in practice.
///
/// # Safety contract (upheld by the compiler)
/// `data` is null, or points to `len` readable bytes for the duration of this call.
fn bytes_to_string(data: *const u8, len: i64) -> String {
    String::from_utf8_lossy(&copy_bytes(data, len)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc;
    use crate::mem::__alloc;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::os::raw::{c_int, c_void};
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[link(name = "gc")]
    unsafe extern "C" {
        fn GC_gcollect();
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }

    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }

    type Job = Box<dyn FnOnce() + Send>;

    // A single persistent Boehm-registered worker thread runs every GC-touching test
    // body (see the identical rationale in `scheduler`'s tests): funneling fiber work
    // onto one long-lived registered thread keeps Boehm's thread set stable so
    // stop-the-world signalling never targets an exited thread.
    fn gc_worker() -> &'static mpsc::Sender<Job> {
        static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                gc::install_hooks();
                let mut stack_base = GcStackBase {
                    mem_base: ptr::null_mut(),
                };
                unsafe {
                    GC_get_stack_base(&mut stack_base);
                    GC_register_my_thread(&stack_base);
                }
                for job in receiver {
                    job();
                }
            });
            sender
        })
    }

    fn on_gc_thread<F: FnOnce() + Send + 'static>(f: F) {
        let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (done_sender, done_receiver) = mpsc::channel();
        gc_worker()
            .send(Box::new(move || {
                f();
                let _ = done_sender.send(());
            }))
            .unwrap();
        done_receiver.recv().unwrap();
    }

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// Read exactly `buf.len()` bytes, looping over partial reads; errors on early EOF.
    fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) {
        let mut filled = 0;
        while filled < buf.len() {
            let n = stream.read(&mut buf[filled..]).unwrap();
            assert!(n > 0, "unexpected EOF at {filled}/{}", buf.len());
            filled += n;
        }
    }

    #[test]
    fn echo_round_trip_on_one_thread() {
        static SERVER_DONE: AtomicBool = AtomicBool::new(false);
        static CLIENT_DONE: AtomicBool = AtomicBool::new(false);
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        SERVER_DONE.store(false, Ordering::SeqCst);
        CLIENT_DONE.store(false, Ordering::SeqCst);
        GOT.lock().unwrap().clear();

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut buf = [0u8; 4];
                    read_exact(&mut conn, &mut buf);
                    // Echo the request straight back.
                    conn.write_all(&buf).unwrap();
                    SERVER_DONE.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut stream = TcpStream::connect(addr).unwrap();
                    stream.write_all(b"ping").unwrap();
                    let mut buf = [0u8; 4];
                    read_exact(&mut stream, &mut buf);
                    *GOT.lock().unwrap() = buf.to_vec();
                    CLIENT_DONE.store(true, Ordering::SeqCst);
                });
            });
        });

        assert!(SERVER_DONE.load(Ordering::SeqCst), "server fiber finished");
        assert!(CLIENT_DONE.load(Ordering::SeqCst), "client fiber finished");
        assert_eq!(&*GOT.lock().unwrap(), b"ping");
    }

    #[test]
    fn reactor_services_sleep_and_socket_together() {
        // A fiber sleeps while the client/server pair does socket IO: proves one
        // `Poll::poll` services both the timer and socket readiness.
        static SLEPT: AtomicBool = AtomicBool::new(false);
        static ECHOED: AtomicBool = AtomicBool::new(false);
        SLEPT.store(false, Ordering::SeqCst);
        ECHOED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(|| {
                    sleep(Duration::from_millis(30));
                    SLEPT.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut buf = [0u8; 5];
                    read_exact(&mut conn, &mut buf);
                    conn.write_all(&buf).unwrap();
                });

                spawn(move || {
                    // Delay the connect so the sleeper is already parked on a timer
                    // while this fiber parks on socket readiness.
                    sleep(Duration::from_millis(5));
                    let mut stream = TcpStream::connect(addr).unwrap();
                    stream.write_all(b"hello").unwrap();
                    let mut buf = [0u8; 5];
                    read_exact(&mut stream, &mut buf);
                    assert_eq!(&buf, b"hello");
                    ECHOED.store(true, Ordering::SeqCst);
                });
            });
        });

        assert!(SLEPT.load(Ordering::SeqCst), "sleeping fiber woke");
        assert!(ECHOED.load(Ordering::SeqCst), "socket echo completed");
    }

    #[test]
    fn socket_parked_fiber_roots_survive_collection() {
        // A fiber holds the only references to GC allocations on its own stack, then
        // parks on a socket READ (no data yet). While it is socket-parked, a sibling
        // forces a collection; then it sends data, waking the reader, which verifies
        // its objects are byte-for-byte intact — proving socket-parked stacks are
        // scanned exactly like sleep-parked ones.
        const N: usize = 32;
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        let p = __alloc(LEN as i64) as *mut u8;
                        unsafe { ptr::write_bytes(p, (i as u8).wrapping_add(1), LEN) };
                        *slot = p;
                    }
                    let held = std::hint::black_box(held);
                    // Parks here on socket readiness while the client collects.
                    let mut buf = [0u8; 2];
                    read_exact(&mut conn, &mut buf);
                    // Churn the heap to reclaim anything wrongly freed, then verify.
                    for _ in 0..64 {
                        let p = __alloc(LEN as i64) as *mut u8;
                        unsafe { ptr::write_bytes(p, 0xEE, LEN) };
                        std::hint::black_box(p);
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        let want = (i as u8).wrapping_add(1);
                        if (0..LEN).all(|k| unsafe { *p.add(k) } == want) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut stream = TcpStream::connect(addr).unwrap();
                    // Let the server accept, allocate, and park on read.
                    sleep(Duration::from_millis(20));
                    unsafe { GC_gcollect() };
                    stream.write_all(b"go").unwrap();
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }

    #[test]
    fn tcp_request_round_trips_against_a_local_listener() {
        // The whole `@tcpRequest` path end to end: the primitive LAUNCHES a background producer
        // that connects to a real listener, writes the request, and reads the response until the
        // peer closes; a separate fiber FORCES the deferred value; the `Ok(responseBytes)` flows
        // back.
        use crate::deferred::{RESULT_OK_TAG, __force_result};
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener as StdListener;

        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static TAG: AtomicUsize = AtomicUsize::new(0);
        GOT.lock().unwrap().clear();
        TAG.store(usize::MAX, Ordering::SeqCst);

        // A blocking std listener on its own OS thread stands in for a peer: accept one
        // connection, read the request, write a fixed response, then close (close-delimited —
        // dropping the stream is what ends the client's read-to-close).
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = [0u8; 5];
            conn.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"PING\n");
            conn.write_all(b"PONG\n").unwrap();
        });

        let address = format!("{addr}");
        on_gc_thread(move || {
            run(move || {
                let deferred = __tcp_request_launch(
                    address.as_ptr(),
                    address.len() as i64,
                    b"PING\n".as_ptr(),
                    5,
                );
                let deferred_ptr = deferred.slot.data;
                spawn(move || {
                    let forced = __force_result(deferred_ptr);
                    TAG.store(forced.tag as usize, Ordering::SeqCst);
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            forced.slot.data as *const u8,
                            forced.slot.len as usize,
                        )
                    };
                    *GOT.lock().unwrap() = bytes.to_vec();
                });
            });
        });

        server.join().unwrap();
        assert_eq!(TAG.load(Ordering::SeqCst), RESULT_OK_TAG as usize, "Ok variant");
        assert_eq!(&*GOT.lock().unwrap(), b"PONG\n");
    }
}
