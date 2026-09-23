// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Non-blocking TCP for the fiber scheduler: the shared plumbing [`client`] and
//! [`server`] both sit on.
//!
//! [`TcpStream`] wraps a `mio` non-blocking socket and registers it with the reactor's
//! `Poll`. Every op that would block parks the calling fiber
//! (via `crate::scheduler::park_on_readiness`) instead of spinning or blocking the OS
//! thread: it (re)registers the source for the readiness it needs, yields to the
//! scheduler, and is resumed only when the reactor reports that token ready — exactly
//! the way [`crate::scheduler::sleep`] parks on a deadline. Many sockets thus make
//! progress cooperatively on one thread. `TcpListener` is its server-side twin, and
//! `resolve`/`resolve_hostname` turn a `host:port` address into the `SocketAddr`
//! both [`client`] and [`server`] connect or bind to.
//!
//! GC note: parking is transparent to the collector. A parked fiber's stack — with
//! its live roots — is scanned by [`crate::gc`]'s `GC_push_other_roots` callback,
//! which pushes every registered fiber that is not currently running, regardless of
//! *why* it is parked. A socket-blocked fiber is therefore covered identically to a
//! sleeping one; `tests::socket_parked_fiber_roots_survive_collection` proves it.

use crate::blocking::run_blocking;
use crate::scheduler::{
    deregister_readiness, park_on_readiness, park_on_readiness_or_deadline, register_readiness,
    reregister_readiness,
};
use mio::event::Source;
use mio::{Interest, Token};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::Instant;

pub mod client;
pub mod server;

pub use client::__tcp_request_launch;
pub use server::{
    __connection_close, __connection_read_launch, __connection_read_with_timeout_launch,
    __connection_write, __server_address_host, __server_address_port, __server_kill,
    __tcp_serve_launch,
};

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

    /// Read once, parking until readable OR until `deadline` passes, whichever comes
    /// first — `Connection.@read(seconds)`'s own read. `Ok(0)` both at EOF and once the
    /// deadline passes with nothing readable: a caller with no channel to distinguish
    /// "closed" from "timed out" (`Text` carries neither) treats them alike, the same way
    /// a plain [`Self::read`]'s `Ok(0)` already means EOF. Checks the deadline both before
    /// parking (a call already past it never parks at all) and after waking (the wake may
    /// be the deadline itself, or a spurious readiness ping before it) rather than trusting
    /// which one fired — resuming a fiber carries no reason back, so re-checking the clock
    /// is simpler than a scheduler API that would report one.
    pub fn read_with_deadline(&mut self, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
        loop {
            match self.inner.read(buf) {
                Ok(count) => return Ok(count),
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(ref e) if would_block(e) => {
                    if Instant::now() >= deadline {
                        return Ok(0);
                    }
                    reregister_readiness(&mut self.inner, self.token, Interest::READABLE)?;
                    park_on_readiness_or_deadline(self.token, deadline);
                    if Instant::now() >= deadline {
                        return Ok(0);
                    }
                }
                Err(e) => return Err(e),
            }
        }
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

impl TcpStream {
    /// The OS descriptor underneath this stream — read-only, and stable for the stream's
    /// whole life, so a fiber that does not otherwise touch this `TcpStream` (`Server.kill`,
    /// forcibly closing a connection its handler fiber may itself be parked inside a read or
    /// write of) can still act on the connection without a `&mut` that would alias one
    /// already held across that park.
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }

    /// Wrap an already-connected socket (from [`TcpListener::accept`]) as a
    /// reactor-registered stream — the server side's counterpart to [`Self::connect`]'s
    /// client-side handshake, with no handshake of its own left to wait out.
    fn from_accepted(mut inner: mio::net::TcpStream) -> io::Result<TcpStream> {
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpStream { inner, token })
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

/// A non-blocking, reactor-registered TCP listener — [`TcpStream`]'s server-side twin,
/// backing `net.@tcpServe`'s accept loop.
struct TcpListener {
    inner: mio::net::TcpListener,
    token: mio::Token,
}

impl TcpListener {
    /// Bind and start listening on `addr`, registered with the reactor for READABLE
    /// (accept) readiness.
    fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
        let mut inner = mio::net::TcpListener::bind(addr)?;
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpListener { inner, token })
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Accept one connection, parking (via [`io_loop`]) until one is ready.
    fn accept(&mut self) -> io::Result<(TcpStream, SocketAddr)> {
        let (raw, address) = io_loop(&mut self.inner, self.token, Interest::READABLE, |l| {
            l.accept()
        })?;
        Ok((TcpStream::from_accepted(raw)?, address))
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

/// Resolve `address` (`host:port`, e.g. `127.0.0.1:8080` or `example.com:80`) to a single
/// [`SocketAddr`]. A numeric address parses inline with no network call or thread; a hostname
/// resolves on [`resolve_hostname`], off the reactor thread.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    match address.parse() {
        Ok(addr) => Ok(addr),
        Err(_) => resolve_hostname(address, |address| {
            // The real DNS lookup: `ToSocketAddrs`'s blocking `getaddrinfo`, erroring if it
            // names nothing.
            address.to_socket_addrs()?.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "address resolved to no endpoint")
            })
        }),
    }
}

/// `resolve`'s hostname path: run `lookup` on the runtime's blocking-call pool
/// ([`crate::blocking::run_blocking`]) and flatten its outcome into the ordinary
/// resolve-failure shape — a pool failure (thread creation refused, or `lookup` panicked)
/// reads the same as a lookup that itself failed, since either way there is no address.
fn resolve_hostname(
    address: &str,
    lookup: impl FnOnce(&str) -> io::Result<SocketAddr> + Send + 'static,
) -> io::Result<SocketAddr> {
    let address = address.to_string();
    run_blocking(move || lookup(&address)).and_then(std::convert::identity)
}

/// Copy a `Text`'s `len` content bytes at `data`, past its header, into an owned `Vec`.
fn copy_bytes(data: *const u8, len: i64) -> Vec<u8> {
    crate::text::byte_slice(data, len).to_vec()
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
    use crate::gc_test_harness::on_gc_thread;
    use crate::mem::__alloc;
    use crate::scheduler::{run, sleep, spawn};
    use std::net::TcpListener;
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[link(name = "gc", kind = "static")]
    unsafe extern "C" {
        fn GC_gcollect();
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
        static CLIENT_DONE: AtomicBool = AtomicBool::new(false);
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        CLIENT_DONE.store(false, Ordering::SeqCst);
        GOT.lock().unwrap().clear();

        // A blocking std listener on its own OS thread stands in for the peer: accept
        // one connection and echo back whatever it reads.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4];
            conn.read_exact(&mut buf).unwrap();
            conn.write_all(&buf).unwrap();
        });

        on_gc_thread(move || {
            run(move || {
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

        peer.join().unwrap();
        assert!(CLIENT_DONE.load(Ordering::SeqCst), "client fiber finished");
        assert_eq!(&*GOT.lock().unwrap(), b"ping");
    }

    #[test]
    fn reactor_services_sleep_and_socket_together() {
        // A fiber sleeps while a client fiber does socket IO against a peer thread:
        // proves one `Poll::poll` services both the timer and socket readiness.
        static SLEPT: AtomicBool = AtomicBool::new(false);
        static ECHOED: AtomicBool = AtomicBool::new(false);
        SLEPT.store(false, Ordering::SeqCst);
        ECHOED.store(false, Ordering::SeqCst);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 5];
            conn.read_exact(&mut buf).unwrap();
            conn.write_all(&buf).unwrap();
        });

        on_gc_thread(move || {
            run(move || {
                spawn(|| {
                    sleep(Duration::from_millis(30));
                    SLEPT.store(true, Ordering::SeqCst);
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

        peer.join().unwrap();
        assert!(SLEPT.load(Ordering::SeqCst), "sleeping fiber woke");
        assert!(ECHOED.load(Ordering::SeqCst), "socket echo completed");
    }

    #[test]
    fn socket_parked_fiber_roots_survive_collection() {
        // A fiber holds the only references to GC allocations on its own stack, then
        // parks on a socket READ (no data yet). While it is socket-parked, a sibling
        // forces a collection; then the peer sends data, waking the reader, which
        // verifies its objects are byte-for-byte intact — proving socket-parked stacks
        // are scanned exactly like sleep-parked ones.
        const N: usize = 32;
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        // A blocking std listener on its own OS thread stands in for the peer: accept
        // one connection, wait long enough for the fiber below to allocate and park on
        // read, then send the two bytes that wake it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            conn.write_all(b"go").unwrap();
        });

        on_gc_thread(move || {
            run(move || {
                spawn(move || {
                    let mut stream = TcpStream::connect(addr).unwrap();
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        let p = __alloc(LEN as i64) as *mut u8;
                        unsafe { ptr::write_bytes(p, (i as u8).wrapping_add(1), LEN) };
                        *slot = p;
                    }
                    let held = std::hint::black_box(held);
                    // Parks here on socket readiness while a sibling fiber collects.
                    let mut buf = [0u8; 2];
                    read_exact(&mut stream, &mut buf);
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

                spawn(|| {
                    // Let the sibling connect, allocate, and park on read.
                    sleep(Duration::from_millis(10));
                    unsafe { GC_gcollect() };
                });
            });
        });

        peer.join().unwrap();
        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }

    #[test]
    fn hostname_resolution_parks_the_fiber_without_blocking_the_scheduler() {
        // The lookup hook blocks until a sibling fiber on the same scheduler has run and
        // signaled back over a channel. That rendezvous is only satisfiable if the scheduler
        // thread stays free to run the sibling while the lookup is in flight — exactly what
        // running the lookup on a helper thread buys. Under the old code, where the lookup ran
        // synchronously on the fiber/scheduler thread, the sibling would never get to run and
        // this would deadlock; `recv_timeout` turns that into a clear test failure instead of
        // hanging the suite.
        static RESOLVED: AtomicBool = AtomicBool::new(false);
        RESOLVED.store(false, Ordering::SeqCst);

        let (sibling_ran_sender, sibling_ran_receiver) = mpsc::channel::<()>();

        on_gc_thread(move || {
            run(move || {
                spawn(move || {
                    let lookup = move |_: &str| {
                        // Only satisfiable if the sibling below actually ran while this lookup
                        // was in flight — impossible under the old, fiber-thread-blocking code.
                        sibling_ran_receiver
                            .recv_timeout(Duration::from_secs(5))
                            .expect("sibling fiber never ran while the lookup was in flight");
                        Ok(SocketAddr::from(([127, 0, 0, 1], 0)))
                    };
                    let resolved = resolve_hostname("example.invalid:80", lookup);
                    assert!(resolved.is_ok(), "the lookup resolved");
                    RESOLVED.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let _ = sibling_ran_sender.send(());
                });
            });
        });

        assert!(RESOLVED.load(Ordering::SeqCst), "the resolve completed");
    }

    #[test]
    fn resolve_hostname_round_trips_the_lookups_result() {
        // A basic sanity check on the ordinary (non-adversarial) path: `resolve_hostname`
        // delivers the lookup's own answer back to the calling fiber. The blocking-call pool's
        // own mechanics (concurrency, growth and its ceiling, a panicking job) are
        // `crate::blocking`'s tests, driven directly against the pool with plain closures —
        // this one only exercises `resolve_hostname`'s own plumbing atop it.
        static RESOLVED_ADDRESS: Mutex<Option<SocketAddr>> = Mutex::new(None);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let expected = SocketAddr::from(([127, 0, 0, 1], 4242));
                    let lookup = move |_: &str| Ok(expected);
                    let resolved = resolve_hostname("example.invalid:80", lookup)
                        .expect("the lookup resolved");
                    *RESOLVED_ADDRESS.lock().unwrap() = Some(resolved);
                });
            });
        });

        assert_eq!(
            *RESOLVED_ADDRESS.lock().unwrap(),
            Some(SocketAddr::from(([127, 0, 0, 1], 4242)))
        );
    }
}
