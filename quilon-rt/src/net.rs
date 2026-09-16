// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Non-blocking TCP for the fiber scheduler.
//!
//! [`TcpStream`] wraps a `mio` non-blocking socket and registers it with the reactor's
//! `Poll`. Every op that would block parks the calling fiber
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
//! failure terminates the process; the outcome flows back to `.qn` code to match on. It is
//! internal — the HTTP client sits on it; users do not import raw sockets.
//!
//! GC note: parking is transparent to the collector. A parked fiber's stack — with
//! its live roots — is scanned by [`crate::gc`]'s `GC_push_other_roots` callback,
//! which pushes every registered fiber that is not currently running, regardless of
//! *why* it is parked. A socket-blocked fiber is therefore covered identically to a
//! sleeping one; `tests::socket_parked_fiber_roots_survive_collection` proves it.

use crate::blocking::run_blocking;
use crate::deferred::{QlResult, launch, launch_deferred_result, launch_deferred_text, settle};
use crate::mem::{QlSlice, alloc_text, format_num};
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
use crate::scheduler::{
    deregister_readiness, park_on_readiness, register_readiness, reregister_readiness, sleep, spawn,
};
use mio::event::Source;
use mio::{Interest, Token};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::os::raw::c_void;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

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
/// collection might reclaim. The deferred `Result` is written into `out` rather than returned:
/// a `Result` is 24 bytes, which the C ABI returns via a hidden pointer, so an out-pointer keeps
/// the FFI boundary free of an aggregate return (see [`__force_result`]).
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QlResult`]; `address_data`/`request_data` are null,
/// or point to `address_len`/`request_len` readable bytes for the duration of this call (a
/// `Text`'s live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_request_launch(
    out: *mut QlResult,
    address_data: *const u8,
    address_len: i64,
    request_data: *const u8,
    request_len: i64,
) {
    let address = bytes_to_string(address_data, address_len);
    let request = copy_bytes(request_data, request_len);
    let deferred = launch_deferred_result(move || tcp_request(&address, &request));
    // SAFETY: `out` is writable storage for one `QlResult` (the code generator's alloca).
    unsafe { *out = deferred };
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
    QlResult::not_ok(&format!(
        "@tcpRequest to {address} failed at {stage}: {error}"
    ))
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

// --- net.@tcpServe: the raw TCP server layer -------------------------------------------
//
// The runtime owns only what the language cannot express: accepting connections and
// running each on its own fiber. `Connection`/`Server` are opaque handles — a `Num` id
// into the thread-local tables below — with every method compiler-lowered (see
// `src/codegen/generator/calls.rs`'s `generate_at_primitive` and its `close`/`kill`
// interception ahead of ordinary method dispatch).
//
// `net.@tcpServe`'s accept loop is a launch registered directly with `launch_scope`
// (through the generic `deferred::launch`, whose own return value — the deferred cell
// pointer — is kept on `ServerState` rather than exposed to Quilon, since `@tcpServe`'s
// OWN return value, the `Server` handle, is a ready `Num` built by codegen, never
// deferred). `Server.kill` settles that same cell before returning, so the enclosing
// block's own join finds it already done.

/// A connection's own state: the accepted stream (taken on close, so a further read/write
/// after that reads as "closed" rather than reusing a dropped socket), its raw descriptor
/// (kept outside the `RefCell` so `Server.kill` can shut it down without contending with a
/// handler fiber's read/write, which holds the `RefCell` borrow across a park), and the
/// server's own open-connection set, so closing removes this connection's id from it.
struct ConnectionState {
    stream: RefCell<Option<TcpStream>>,
    raw_fd: RawFd,
    open_connections: Rc<RefCell<HashSet<u64>>>,
}

/// A server's own state, reachable both from its accept loop (owns nothing here directly —
/// the listener lives on the loop's own closure, dropped when it returns) and from
/// `Server.kill`, run on whichever fiber calls it.
struct ServerState {
    stopping: Rc<Cell<bool>>,
    in_flight: Rc<Cell<usize>>,
    open_connections: Rc<RefCell<HashSet<u64>>>,
    /// The accept loop's own deferred cell, from the `deferred::launch` call that started
    /// it — never exposed to Quilon; `Server.kill` [`settle`]s it so the enclosing block's
    /// own join (over the SAME cell, registered by that same `launch` call) finds it
    /// already done.
    accept_loop: *mut crate::deferred::Deferred<()>,
    /// Where the listener is actually bound — used to wake the accept loop's own parked
    /// `accept()` on `kill` (see [`wake_accept_loop`]).
    local_addr: SocketAddr,
}

thread_local! {
    static NEXT_HANDLE: Cell<u64> = const { Cell::new(1) };
    static CONNECTIONS: RefCell<HashMap<u64, Rc<ConnectionState>>> = RefCell::new(HashMap::new());
    static SERVERS: RefCell<HashMap<u64, Rc<ServerState>>> = RefCell::new(HashMap::new());
}

/// A fresh handle id, never reused — ids are never freed back into a pool, so a stale
/// `Connection`/`Server` value (one a program held onto past its own close/kill) can never
/// be confused with a later, unrelated one.
fn next_handle() -> u64 {
    NEXT_HANDLE.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

/// A `net.@tcpServe` port argument as the whole `0..=65535` number it must be, or the
/// message saying why it is not — folded into the same bind-failure report as an OS-level
/// bind error, since a program has no other channel to hear about either.
fn parse_port(port: f64) -> Result<u16, String> {
    if port.fract() != 0.0 || port < 0.0 || port > u16::MAX as f64 {
        return Err(format!(
            "port must be a whole number from 0 to 65535, got {}",
            format_num(port)
        ));
    }
    Ok(port as u16)
}

/// `net.@tcpServe(port, handler)`: bind `0.0.0.0:port`, listen, and return the `Server`
/// handle's id at once — codegen builds the `Server { handle = … }` record around it, the
/// same way it builds the `Connection` handed to `handler`. The accept loop launches on a
/// background fiber, registered with whatever `< >` block's launch scope is open right now
/// (through [`launch`], exactly as a value-returning primitive's producer registers) so
/// that block's own join keeps it alive without ever forcing a value from this call — this
/// call's own return is already ready. A bind failure is fatal, reported at `site` through
/// the same fail-loud path every other unrecoverable runtime check uses.
///
/// # Safety contract (upheld by the compiler)
/// `handler_fn` is a live `(f64, ptr) -> i8` trampoline — codegen's fixed-shape wrapper
/// (see `CodeGenerator::emit_tcp_serve_handler_thunk`) over the user's `(Connection) -> $`
/// closure — called with `handler_env` as its second argument; `site` is null or points to
/// a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_launch(
    port: f64,
    handler_fn: *const c_void,
    handler_env: *mut c_void,
    site: *const QlSite,
) -> f64 {
    // SAFETY: per the contract, a live `(f64, ptr) -> i8` trampoline.
    let handler_fn: extern "C" fn(f64, *mut c_void) -> u8 =
        unsafe { std::mem::transmute(handler_fn) };

    let bind = |reason: String| -> ! {
        fail_at(
            site,
            codes::BIND_FAILED,
            &format!("@tcpServe: bind 0.0.0.0:{}: {reason}", format_num(port)),
            RUNTIME_EXIT_CODE,
        )
    };
    let port_number = match parse_port(port) {
        Ok(number) => number,
        Err(reason) => bind(reason),
    };
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port_number);
    let mut listener = match TcpListener::bind(bind_addr) {
        Ok(listener) => listener,
        Err(error) => bind(error.to_string()),
    };
    let local_addr = listener.local_addr().unwrap_or(bind_addr);

    let stopping = Rc::new(Cell::new(false));
    let in_flight = Rc::new(Cell::new(0usize));
    let open_connections: Rc<RefCell<HashSet<u64>>> = Rc::new(RefCell::new(HashSet::new()));

    let loop_stopping = Rc::clone(&stopping);
    let loop_in_flight = Rc::clone(&in_flight);
    let loop_open_connections = Rc::clone(&open_connections);
    let accept_loop = launch(move || {
        loop {
            let stream = match listener.accept() {
                Ok((stream, _peer)) => stream,
                // A genuine accept error (not one `kill` caused) ends the loop; connections
                // already handed to handlers keep running on their own fibers regardless.
                Err(_) => break,
            };
            if loop_stopping.get() {
                drop(stream);
                break;
            }
            spawn_connection_handler(
                stream,
                handler_fn,
                handler_env,
                &loop_in_flight,
                &loop_open_connections,
            );
        }
        // `listener` drops here, deregistering it — the runtime's own "close the listener".
    });

    let server_id = next_handle();
    SERVERS.with(|servers| {
        servers.borrow_mut().insert(
            server_id,
            Rc::new(ServerState {
                stopping,
                in_flight,
                open_connections,
                accept_loop,
                local_addr,
            }),
        );
    });
    server_id as f64
}

/// Register `stream` as an open connection and run `handler_fn` against it on its own
/// fiber: the accept loop's per-connection half. Auto-closes the connection once the
/// handler returns (`close-after-handler is the runtime's`, whether or not the handler
/// closed it itself) and only then leaves `in_flight`, so `Server.kill`'s wait sees this
/// handler as in-flight for its whole run, not just until its connection closes.
fn spawn_connection_handler(
    stream: TcpStream,
    handler_fn: extern "C" fn(f64, *mut c_void) -> u8,
    handler_env: *mut c_void,
    in_flight: &Rc<Cell<usize>>,
    open_connections: &Rc<RefCell<HashSet<u64>>>,
) {
    let id = next_handle();
    let raw_fd = stream.as_raw_fd();
    CONNECTIONS.with(|connections| {
        connections.borrow_mut().insert(
            id,
            Rc::new(ConnectionState {
                stream: RefCell::new(Some(stream)),
                raw_fd,
                open_connections: Rc::clone(open_connections),
            }),
        );
    });
    open_connections.borrow_mut().insert(id);
    in_flight.set(in_flight.get() + 1);

    let in_flight = Rc::clone(in_flight);
    spawn(move || {
        handler_fn(id as f64, handler_env);
        close_connection(id);
        in_flight.set(in_flight.get() - 1);
    });
}

/// Close connection `id` if it is still open (idempotent: a no-op for one already closed,
/// by an earlier `Connection.close()` or the runtime's own close-after-handler) — takes the
/// stream out of its table entry, which drops and deregisters it, and removes `id` from its
/// server's open-connection set. Reused by `Connection.close()` and the post-handler
/// auto-close.
fn close_connection(id: u64) {
    let Some(state) = CONNECTIONS.with(|connections| connections.borrow_mut().remove(&id)) else {
        return;
    };
    state.open_connections.borrow_mut().remove(&id);
    state.stream.borrow_mut().take();
}

/// Shut connection `id` down at the descriptor level, without touching its `RefCell` —
/// `Server.kill`'s force-close of a connection still open past its grace period, called
/// from a DIFFERENT fiber than the one whose handler may be parked mid-read/write on this
/// same connection (holding that `RefCell` borrow across the park). `shutdown(2)` needs no
/// such borrow: it acts on the descriptor directly, which is what lets it interrupt that
/// parked read/write rather than deadlock behind it — the parked side typically wakes with
/// EOF or a write error and finishes on its own; the descriptor itself is only actually
/// closed once that side (or a later `Connection.close()`) drops the `TcpStream`.
///
/// ponytail: a permanently stuck handler (one that never touches this connection
/// again after the shutdown) leaves that drop — and its fiber's stack — unreclaimed
/// forever; accepted here the way every launch on this tier is never cancelled. A
/// fiber-cancellation primitive would let this reclaim the stack instead of leaking it.
fn force_shutdown_connection(id: u64) {
    let Some(state) = CONNECTIONS.with(|connections| connections.borrow().get(&id).cloned()) else {
        return;
    };
    // SAFETY: `raw_fd` names a socket this process owns for as long as the connection's
    // table entry exists, which this call holds a clone of; `shutdown` only changes the
    // socket's protocol state, never its descriptor's validity.
    unsafe {
        libc::shutdown(state.raw_fd, libc::SHUT_RDWR);
    }
}

/// `Connection.@read()`: launch a background read of whatever bytes have arrived and
/// return the deferred `Text` immediately — the connection's counterpart to `@readStdin`,
/// sharing the same generic deferral core and force path (`__force_text`). `""` once the
/// connection is closed (by the peer, `Connection.close()`, or `Server.kill`) — a genuine
/// read error reads the same way, since `Text` carries no channel to report one and no
/// per-connection failure is fatal on this layer.
#[unsafe(no_mangle)]
pub extern "C" fn __connection_read_launch(connection_id: f64) -> QlSlice {
    let id = connection_id as u64;
    launch_deferred_text(move || read_connection_once(id))
}

/// Read once from connection `id`'s stream, parking on readiness until data or EOF/an error
/// arrives. `""` when the connection has already closed, at EOF, or on any read error.
fn read_connection_once(id: u64) -> QlSlice {
    let Some(state) = CONNECTIONS.with(|connections| connections.borrow().get(&id).cloned()) else {
        return alloc_text(&[]);
    };
    let mut slot = state.stream.borrow_mut();
    let Some(stream) = slot.as_mut() else {
        return alloc_text(&[]);
    };
    let mut buffer = [0u8; 4096];
    match stream.read(&mut buffer) {
        Ok(count) => alloc_text(&buffer[..count]),
        Err(_) => alloc_text(&[]),
    }
}

/// `Connection.@write(bytes)`: write every byte, parking on writability until all of it is
/// sent. Effect-only (`-> $`), so a write past a closed connection — or one that fails
/// partway — is a silent no-op rather than a fault: neither channel this call has (a
/// missing table entry, a `$` return) can carry a reason, and no per-connection failure is
/// fatal on this layer.
///
/// # Safety contract (upheld by the compiler)
/// `data` is null, or points to `len` readable bytes for the duration of this call.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __connection_write(connection_id: f64, data: *const u8, len: i64) {
    let id = connection_id as u64;
    let bytes = copy_bytes(data, len);
    let Some(state) = CONNECTIONS.with(|connections| connections.borrow().get(&id).cloned()) else {
        return;
    };
    let mut slot = state.stream.borrow_mut();
    let Some(stream) = slot.as_mut() else {
        return;
    };
    let _ = stream.write_all(&bytes);
}

/// `Connection.close()`: close the connection now, if it is not already closed.
#[unsafe(no_mangle)]
pub extern "C" fn __connection_close(connection_id: f64) {
    close_connection(connection_id as u64);
}

/// Wake a server's accept loop out of a parked `accept()` with nothing pending, so it
/// notices `stopping` promptly instead of waiting for the next real client (which may never
/// come): connect to the listener's own bound port from loopback. Best-effort — a failure
/// here just means a genuine client connection (or `Server.kill`'s own grace period ending)
/// wakes it instead; either way `kill` still [`settle`]s the accept loop before returning.
fn wake_accept_loop(server: &ServerState) {
    let target = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), server.local_addr.port());
    let _ = TcpStream::connect(target);
}

/// Park the calling fiber, in short sleeps, until `server`'s `in_flight` count reaches zero
/// or `seconds` have elapsed — `Server.kill`'s graceful wait.
///
/// ponytail: a fixed-tick poll rather than a wake on the last handler's own finish — the
/// simplest correct wait, at the cost of up to one tick of latency past the last handler
/// actually finishing. Upgrade to a `park_on_address`/`wake_address` pair keyed by the
/// server if that latency ever matters.
fn wait_for_in_flight(server: &ServerState, seconds: f64) {
    const TICK: Duration = Duration::from_millis(20);
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    while server.in_flight.get() > 0 && Instant::now() < deadline {
        sleep(TICK);
    }
}

/// `Server.kill(seconds)`: stop accepting, wait up to `seconds` for in-flight handlers to
/// finish, force-close any connection still open past that grace period, then settle the
/// accept loop's own launch so the enclosing block's join finds it already done. Parks the
/// calling fiber for as long as any of that takes. A no-op on an already-killed or unknown
/// handle.
#[unsafe(no_mangle)]
pub extern "C" fn __server_kill(server_id: f64, seconds: f64) {
    let id = server_id as u64;
    let Some(server) = SERVERS.with(|servers| servers.borrow_mut().remove(&id)) else {
        return;
    };
    server.stopping.set(true);
    wake_accept_loop(&server);
    wait_for_in_flight(&server, seconds);
    let stuck: Vec<u64> = server.open_connections.borrow().iter().copied().collect();
    for connection_id in stuck {
        force_shutdown_connection(connection_id);
    }
    // SAFETY: `accept_loop` is the cell `launch` returned for this server's own accept
    // loop, still reachable through `server` (this function's only owner of it, since the
    // handle was just removed from `SERVERS` above).
    unsafe {
        settle(server.accept_loop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::__force_text;
    use crate::gc;
    use crate::mem::__alloc;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::net::TcpListener;
    use std::os::raw::{c_int, c_void};
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[link(name = "gc", kind = "static")]
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
    fn tcp_request_round_trips_against_a_local_listener() {
        // The whole `@tcpRequest` path end to end: the primitive LAUNCHES a background producer
        // that connects to a real listener, writes the request, and reads the response until the
        // peer closes; a separate fiber FORCES the deferred value; the `Ok(responseBytes)` flows
        // back.
        use crate::deferred::{__force_result, RESULT_OK_TAG};
        use crate::mem::QlSlice;

        // A zeroed `Result` out-parameter for the FFI calls to fill.
        let blank = || QlResult {
            tag: 0,
            slot: QlSlice::empty(),
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

    // --- net.@tcpServe -------------------------------------------------------------

    /// Where `net.@tcpServe` actually bound `server_id` — a whitebox peek at the private
    /// table, standing in for the ephemeral port a test client needs (`Server` carries no
    /// `.port()` a Quilon program could read either).
    fn bound_addr(server_id: f64) -> SocketAddr {
        SERVERS.with(|servers| {
            servers
                .borrow()
                .get(&(server_id as u64))
                .expect("server handle")
                .local_addr
        })
    }

    /// Force `__connection_read_launch`'s deferred `Text` the way generated code does:
    /// extract the promise from its sentinel-tagged slice and force it.
    fn force_connection_read(connection_id: f64) -> Vec<u8> {
        let deferred = __connection_read_launch(connection_id);
        let forced = __force_text(deferred.data);
        crate::text::byte_slice(forced.data as *const u8, forced.len).to_vec()
    }

    /// `__connection_write`'s `data`/`len` are a `Text`'s own raw fields — header included,
    /// as codegen extracts them from a real `bytes :: Text` argument — so a test calling it
    /// directly (with no codegen in the loop) builds one the same way `text_of_bytes` builds
    /// every other low-level `Text` fixture in this crate's tests, rather than handing it a
    /// bare buffer's pointer.
    fn write_connection_bytes(connection_id: f64, bytes: &[u8]) {
        let (ptr, len) = crate::test_support::text_of_bytes(bytes);
        __connection_write(connection_id, ptr, len);
    }

    /// A std client with a bounded read/write timeout on every socket op it does — every
    /// test below connects this way, so a bug that leaves a connection open with nothing
    /// arriving fails the test in a few seconds instead of hanging the run.
    fn connect_with_timeout(addr: SocketAddr) -> std::net::TcpStream {
        let stream = std::net::TcpStream::connect(addr).expect("connect to the test server");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set a read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("set a write timeout");
        stream
    }

    #[test]
    fn accept_and_handler_per_connection_with_two_concurrent_clients() {
        // Each accepted connection reads one message and echoes it straight back, then
        // returns (letting the runtime auto-close it) — proving the accept loop hands each
        // connection to its OWN fiber, running two clients' exchanges concurrently rather
        // than serializing them.
        extern "C" fn echo_handler(connection_id: f64, _environment: *mut c_void) -> u8 {
            let bytes = force_connection_read(connection_id);
            write_connection_bytes(connection_id, &bytes);
            0
        }

        static FINISHED: AtomicUsize = AtomicUsize::new(0);
        FINISHED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = __tcp_serve_launch(
                        0.0,
                        echo_handler as *const c_void,
                        ptr::null_mut(),
                        ptr::null(),
                    );
                    let addr = bound_addr(server_id);

                    let clients: Vec<_> = [*b"first-", *b"second"]
                        .into_iter()
                        .map(|message: [u8; 6]| {
                            std::thread::spawn(move || {
                                let mut stream = connect_with_timeout(addr);
                                stream.write_all(&message).expect("write the message");
                                let mut echoed = [0u8; 6];
                                stream.read_exact(&mut echoed).expect("read the echo");
                                assert_eq!(echoed, message, "the echoed bytes matched");
                                FINISHED.fetch_add(1, Ordering::SeqCst);
                            })
                        })
                        .collect();

                    let deadline = Instant::now() + Duration::from_secs(10);
                    while FINISHED.load(Ordering::SeqCst) < clients.len()
                        && Instant::now() < deadline
                    {
                        sleep(Duration::from_millis(5));
                    }
                    for client in clients {
                        client.join().expect("client thread panicked");
                    }
                    __server_kill(server_id, 1.0);
                });
            });
        });

        assert_eq!(FINISHED.load(Ordering::SeqCst), 2, "both clients finished");
    }

    #[test]
    fn a_handler_that_forgets_to_close_still_has_its_connection_closed() {
        // A handler that returns without ever calling `close()` — the runtime closes the
        // connection anyway: the client's own read sees EOF rather than hanging.
        extern "C" fn forgetful_handler(_connection_id: f64, _environment: *mut c_void) -> u8 {
            0
        }

        static SAW_EOF: AtomicBool = AtomicBool::new(false);
        SAW_EOF.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = __tcp_serve_launch(
                        0.0,
                        forgetful_handler as *const c_void,
                        ptr::null_mut(),
                        ptr::null(),
                    );
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        let mut buffer = [0u8; 1];
                        let read = stream
                            .read(&mut buffer)
                            .expect("read after the handler returns");
                        SAW_EOF.store(read == 0, Ordering::SeqCst);
                    });

                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !SAW_EOF.load(Ordering::SeqCst) && Instant::now() < deadline {
                        sleep(Duration::from_millis(5));
                    }
                    client.join().expect("client thread panicked");
                    __server_kill(server_id, 1.0);
                });
            });
        });

        assert!(
            SAW_EOF.load(Ordering::SeqCst),
            "the connection closed on its own"
        );
    }

    #[test]
    fn kill_waits_for_an_in_flight_handler_to_finish_before_closing() {
        // The handler deliberately runs past `Server.kill`'s own call, inside its grace
        // period: the client must still get its normal echoed response, proving `kill`
        // waited for it rather than cutting it off.
        extern "C" fn slow_echo_handler(connection_id: f64, _environment: *mut c_void) -> u8 {
            sleep(Duration::from_millis(150));
            let bytes = force_connection_read(connection_id);
            write_connection_bytes(connection_id, &bytes);
            0
        }

        static ECHOED: AtomicBool = AtomicBool::new(false);
        ECHOED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = __tcp_serve_launch(
                        0.0,
                        slow_echo_handler as *const c_void,
                        ptr::null_mut(),
                        ptr::null(),
                    );
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        stream.write_all(b"hold on").expect("write the message");
                        let mut echoed = [0u8; 7];
                        stream.read_exact(&mut echoed).expect("read the echo");
                        ECHOED.store(&echoed == b"hold on", Ordering::SeqCst);
                    });

                    // Give the handler time to accept and start its own sleep before kill
                    // is called, so kill's grace period genuinely overlaps in-flight work.
                    sleep(Duration::from_millis(30));
                    __server_kill(server_id, 5.0);
                    client.join().expect("client thread panicked");
                });
            });
        });

        assert!(
            ECHOED.load(Ordering::SeqCst),
            "kill let the in-flight handler finish and reply"
        );
    }

    #[test]
    fn kill_times_out_and_closes_a_connection_stuck_on_a_read() {
        // The handler parks forever on a read nothing ever answers; `kill`'s short grace
        // period elapses without the handler finishing, so it force-closes the connection —
        // the client's own read sees the connection end rather than hanging.
        extern "C" fn stuck_handler(connection_id: f64, _environment: *mut c_void) -> u8 {
            let _ = force_connection_read(connection_id);
            0
        }

        static CONNECTION_ENDED: AtomicBool = AtomicBool::new(false);
        CONNECTION_ENDED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = __tcp_serve_launch(
                        0.0,
                        stuck_handler as *const c_void,
                        ptr::null_mut(),
                        ptr::null(),
                    );
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        // Never writes anything: the handler's read has nothing to answer it.
                        let mut buffer = [0u8; 1];
                        let outcome = stream.read(&mut buffer);
                        // A forced shutdown reads back as EOF (`Ok(0)`) on this platform, or
                        // a reset — either way the connection did not stay open forever.
                        let ended = matches!(outcome, Ok(0)) || outcome.is_err();
                        CONNECTION_ENDED.store(ended, Ordering::SeqCst);
                    });

                    // Give the handler time to accept and park on its own read before kill's
                    // short grace period elapses.
                    sleep(Duration::from_millis(30));
                    __server_kill(server_id, 0.1);
                    client.join().expect("client thread panicked");
                });
            });
        });

        assert!(
            CONNECTION_ENDED.load(Ordering::SeqCst),
            "the stuck connection was force-closed"
        );
    }
}
