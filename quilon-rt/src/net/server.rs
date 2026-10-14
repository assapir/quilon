// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! `net.@tcpServe`: the raw TCP server layer.
//!
//! The runtime owns only what the language cannot express: accepting connections and
//! running each on its own fiber. `Connection`/`Server` are opaque handles — a `Num` id —
//! with every method compiler-lowered (see `src/codegen/generator/calls.rs`'s
//! `generate_at_primitive` and its `close`/`kill` interception ahead of ordinary method
//! dispatch).
//!
//! **Ownership across workers.** A connection's `ConnectionState` — the live transport,
//! registered with its own worker's reactor — is touched only from the worker that accepted
//! it (its handler fiber, or the runtime's own auto-close once that fiber returns), in that
//! worker's own `connections` table. `Server.kill`, which may run on any worker, never
//! reaches into another worker's table: it force-closes a still-open connection through
//! `ServerState::open_connections`, a shared id-to-raw-descriptor map `shutdown(2)` needs no
//! more than, rather than the worker-owned stream.
//!
//! A `Server`'s `ServerState` is reachable from any worker the same way: its fields are
//! `Arc`/atomic, and the table mapping every live server's id to its state is
//! `crate::placement::Registry::servers`, shared by every worker of this run.
//!
//! A server may bind several listeners (`net.@tcpServe`/`http.@serve`'s listener-list
//! overload), `Plain` or `TLS`, each with its own accept loop (`launch_here`, never moved);
//! `Server.kill` settles every one before returning, so the enclosing join finds them done.

use super::tls::{Identity as TLSIdentity, handshake_server, server_config};
use super::{TcpListener, TcpStream, bytes_to_string, copy_bytes, resolve};
use crate::deferred::{launch_deferred_text_here, launch_here, settle};
use crate::mem::{QnSlice, alloc_text};
use crate::report::{QnSite, RUNTIME_EXIT_CODE, codes, fail_at};
use crate::scheduler::{current_fiber_id, sleep};
use crate::worker::with_worker;
use rustls::ServerConfig;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::raw::c_void;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Whether a listener is `Plain` or `TLS`, carrying `TLS`'s shared `ServerConfig`.
#[derive(Clone)]
pub(crate) enum ListenerKind {
    Plain,
    Tls(Arc<ServerConfig>),
}

/// An accepted connection's transport: a plain stream, or a TLS session past its handshake.
enum ConnectionTransport {
    Plain(TcpStream),
    Tls {
        stream: TcpStream,
        connection: Box<rustls::ServerConnection>,
    },
}

impl ConnectionTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls { stream, connection } => {
                rustls::Stream::new(&mut **connection, stream).read(buf)
            }
        }
    }

    /// [`Self::read`]'s timed counterpart. Checks rustls's own buffered plaintext first (a
    /// pure in-memory read) since a handshake's last flight can already carry a request.
    fn read_with_deadline(&mut self, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read_with_deadline(buf, deadline),
            Self::Tls { stream, connection } => {
                match connection.reader().read(buf) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    other => return other,
                }
                if !stream.wait_readable(deadline)? {
                    return Ok(0);
                }
                rustls::Stream::new(&mut **connection, stream).read(buf)
            }
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(buf),
            Self::Tls { stream, connection } => {
                rustls::Stream::new(&mut **connection, stream).write_all(buf)
            }
        }
    }
}

/// A connection's own state — see the module doc's ownership-rule paragraph.
pub(crate) struct ConnectionState {
    stream: RefCell<Option<ConnectionTransport>>,
    open_connections: Arc<Mutex<HashMap<u64, RawFd>>>,
}

/// One listener within a server: its own accept loop (dropped, deregistering the listener,
/// once the loop returns) and where it actually bound.
struct ListenerState {
    /// The accept loop's own deferred cell; `Server.kill` [`settle`]s every listener's.
    accept_loop: *mut crate::deferred::Deferred<()>,
    /// Where the listener is bound — wakes its parked `accept()` on `kill`.
    local_addr: SocketAddr,
}

/// A server's own state, shared across every listener in it — see the module doc.
pub(crate) struct ServerState {
    stopping: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    /// Every open connection's raw descriptor, for `Server.kill` to force-close from any worker.
    open_connections: Arc<Mutex<HashMap<u64, RawFd>>>,
    listeners: Vec<ListenerState>,
}

// SAFETY: `accept_loop` is a raw pointer only because raw pointers aren't auto Send/Sync;
// every access goes through `deferred::settle`'s own `Mutex`, same as the `Deferred` itself.
unsafe impl Send for ServerState {}
unsafe impl Sync for ServerState {}

/// A fresh `Connection`/`Server` handle id, never reused — see
/// `crate::placement::Registry::next_handle`'s own doc.
fn next_handle() -> u64 {
    with_worker(|worker| worker.registry.next_handle())
}

/// `net.@tcpServe(address, handler)`: resolve `address` exactly as `resolve` resolves
/// `@tcpRequest`'s own, bind and listen on it, and return the `Server` handle's id at once
/// — codegen builds the `Server { handle = … }` record around it. A bind failure is fatal,
/// reported at `site`, naming `address` as written.
///
/// # Safety contract (upheld by the compiler)
/// `address_data` is null, or points to `address_len` readable bytes for the duration of
/// this call (a `Text`'s live bytes at the call site); `handler_fn` is a live `(f64, ptr)
/// -> i8` trampoline — codegen's fixed-shape wrapper over the user's `(Connection) -> $`
/// closure — called with `handler_env` as its second argument; `site` is null or points to
/// a valid [`QnSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_launch(
    address_data: *const u8,
    address_len: i64,
    handler_fn: *const c_void,
    handler_env: *mut c_void,
    site: *const QnSite,
    handshake_timeout_seconds: f64,
) -> f64 {
    let address = bytes_to_string(address_data, address_len);
    // SAFETY: per the contract, a live `(f64, ptr) -> i8` trampoline.
    let handler_fn: extern "C" fn(f64, *mut c_void) -> u8 =
        unsafe { std::mem::transmute(handler_fn) };

    let stopping = Arc::new(AtomicBool::new(false));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let open_connections: Arc<Mutex<HashMap<u64, RawFd>>> = Arc::new(Mutex::new(HashMap::new()));
    let server_identity = Arc::as_ptr(&in_flight) as usize;

    let listener = match start_listener(
        &address,
        ListenerKind::Plain,
        handler_fn,
        handler_env,
        &stopping,
        &in_flight,
        &open_connections,
        server_identity,
        handshake_timeout_seconds,
    ) {
        Ok(listener) => listener,
        Err(reason) => fail_at(
            site,
            codes::BIND_FAILED,
            &format!("core.net.@tcpServe: bind {address}: {reason}"),
            RUNTIME_EXIT_CODE,
        ),
    };

    let server_id = next_handle();
    with_worker(|worker| {
        worker
            .registry
            .servers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                server_id,
                Arc::new(ServerState {
                    stopping,
                    in_flight,
                    open_connections,
                    listeners: vec![listener],
                }),
            );
    });
    server_id as f64
}

/// Resolve and bind `address`, launch its `ListenerKind`-appropriate accept loop
/// (`launch_here`, on the calling worker — see the module doc), and return its
/// [`ListenerState`] — the common half of [`__tcp_serve_launch`] and
/// [`__tcp_serve_listeners_launch`], sharing one server's `stopping`/`in_flight`/
/// `open_connections` across however many listeners call this. `Err(reason)` on a
/// resolve/bind failure; the caller names which listener this was.
#[allow(clippy::too_many_arguments)]
fn start_listener(
    address: &str,
    kind: ListenerKind,
    handler_fn: extern "C" fn(f64, *mut c_void) -> u8,
    handler_env: *mut c_void,
    stopping: &Arc<AtomicBool>,
    in_flight: &Arc<AtomicUsize>,
    open_connections: &Arc<Mutex<HashMap<u64, RawFd>>>,
    server_identity: usize,
    handshake_timeout_seconds: f64,
) -> Result<ListenerState, String> {
    let bind_addr = resolve(address).map_err(|error| error.to_string())?;
    let mut listener = TcpListener::bind(bind_addr).map_err(|error| error.to_string())?;
    let local_addr = listener.local_addr().unwrap_or(bind_addr);

    let loop_stopping = Arc::clone(stopping);
    let loop_in_flight = Arc::clone(in_flight);
    let loop_open_connections = Arc::clone(open_connections);
    let accept_loop = launch_here(move || {
        loop {
            let raw_stream = match listener.accept() {
                Ok((raw_stream, _peer)) => raw_stream,
                // A genuine accept error (not one `kill` caused) ends the loop; connections
                // already handed to handlers keep running on their own fibers regardless.
                Err(_) => break,
            };
            if loop_stopping.load(Ordering::SeqCst) {
                drop(raw_stream);
                break;
            }
            place_connection(
                raw_stream,
                &kind,
                handler_fn,
                handler_env,
                Arc::clone(&loop_in_flight),
                Arc::clone(&loop_open_connections),
                server_identity,
                handshake_timeout_seconds,
            );
        }
        // `listener` drops here, deregistering it — the runtime's own "close the listener".
    });

    Ok(ListenerState {
        accept_loop,
        local_addr,
    })
}

/// `core.net.Identity`'s own declaration-order tags (`None`/`Files`/`Base64`) — the
/// `identity_kind` flag codegen flattens an `Identity` sum value into for
/// `__tcp_serve_listeners_push`, mirroring how `deferred::RESULT_OK_TAG`/`RESULT_NOTOK_TAG`
/// mirror `Result`'s own declaration order. `Base64` needs no constant of its own: it is
/// whatever `identity_kind` is once it is neither of these.
const IDENTITY_NONE_TAG: i64 = 0;
const IDENTITY_FILES_TAG: i64 = 1;

/// One `core.net.Listener` [`__tcp_serve_listeners_push`] has read off a `[]Listener`
/// element and recorded. Held rather than reported immediately: `push` has no `site` to
/// report at, only [`__tcp_serve_listeners_launch`] does.
struct ListenerSpec {
    address: String,
    outcome: Result<ListenerSpecKind, bool>,
}

/// A listener's resolved kind, past the mismatch check — [`ListenerKind`]'s own
/// not-yet-built-a-`ServerConfig` counterpart.
enum ListenerSpecKind {
    Plain,
    Tls(TLSIdentity),
}

/// Start a `[]Listener`-list builder for `net.@tcpServe`/`http.@serve`'s listener-list
/// overload: an opaque handle [`__tcp_serve_listeners_push`] appends one listener to at a
/// time, as codegen loops over the `[]Listener` argument, consumed by
/// [`__tcp_serve_listeners_launch`] once the loop ends.
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_listeners_new() -> *mut c_void {
    Box::into_raw(Box::new(Vec::<ListenerSpec>::new())).cast()
}

/// Append one listener — read off a `core.net.Listener` record's `address`/`transport`/
/// `identity` fields by codegen, with `identity`'s own sum flattened into `identity_kind`
/// (see `IDENTITY_NONE_TAG`/`IDENTITY_FILES_TAG`) and its two canonical payload slots,
/// meaningless when `identity_kind` is `None`'s — to `list`.
///
/// # Safety contract (upheld by the compiler)
/// `list` is a pointer [`__tcp_serve_listeners_new`] returned, not yet handed to
/// [`__tcp_serve_listeners_launch`]; `address_data`/`first_data`/`second_data` are each
/// null, or point to their own `_len` readable bytes for the duration of this call.
#[allow(clippy::too_many_arguments, clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_listeners_push(
    list: *mut c_void,
    address_data: *const u8,
    address_len: i64,
    is_tls: i64,
    identity_kind: i64,
    first_data: *const u8,
    first_len: i64,
    second_data: *const u8,
    second_len: i64,
) {
    let address = bytes_to_string(address_data, address_len);
    let is_tls = is_tls != 0;
    let has_identity = identity_kind != IDENTITY_NONE_TAG;
    let outcome = if is_tls != has_identity {
        Err(has_identity)
    } else if !is_tls {
        Ok(ListenerSpecKind::Plain)
    } else if identity_kind == IDENTITY_FILES_TAG {
        Ok(ListenerSpecKind::Tls(TLSIdentity::Files {
            certificate_path: bytes_to_string(first_data, first_len),
            key_path: bytes_to_string(second_data, second_len),
        }))
    } else {
        Ok(ListenerSpecKind::Tls(TLSIdentity::Base64 {
            certificate: bytes_to_string(first_data, first_len),
            key: bytes_to_string(second_data, second_len),
        }))
    };
    // SAFETY: per the contract, a live `Vec<ListenerSpec>` this call's own builder owns.
    let specs = unsafe { &mut *list.cast::<Vec<ListenerSpec>>() };
    specs.push(ListenerSpec { address, outcome });
}

/// Consume `list` (built by zero or more [`__tcp_serve_listeners_push`] calls) and start
/// every listener it named, sharing one `Server` across all of them: `kill` stops every
/// one, `addresses()` reports all of their bound addresses. Builds each `TLS` listener's
/// `ServerConfig` once, here, from its identity, and bounds its handshake by
/// `handshake_timeout_seconds` (`ServerOptions.idleTimeout` for `http.@serve`, the same
/// 5-second default as `ServerOptions.default()` for `net.@tcpServe`). Fatal — naming the
/// failing listener's address and the reason — on a mismatched transport/identity, an
/// identity that fails to load, or a bind failure.
///
/// # Safety contract (upheld by the compiler)
/// `list` is the pointer [`__tcp_serve_listeners_new`] returned for this call, filled by
/// zero or more `__tcp_serve_listeners_push` calls and not consumed anywhere else;
/// `handler_fn`/`handler_env`/`site` are as [`__tcp_serve_launch`]'s own contract.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_listeners_launch(
    list: *mut c_void,
    handler_fn: *const c_void,
    handler_env: *mut c_void,
    site: *const QnSite,
    handshake_timeout_seconds: f64,
) -> f64 {
    // SAFETY: per the contract, the boxed `Vec<ListenerSpec>` this call's own builder made.
    let specs = *unsafe { Box::from_raw(list.cast::<Vec<ListenerSpec>>()) };
    // SAFETY: per the contract, a live `(f64, ptr) -> i8` trampoline.
    let handler_fn: extern "C" fn(f64, *mut c_void) -> u8 =
        unsafe { std::mem::transmute(handler_fn) };

    let stopping = Arc::new(AtomicBool::new(false));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let open_connections: Arc<Mutex<HashMap<u64, RawFd>>> = Arc::new(Mutex::new(HashMap::new()));
    let server_identity = Arc::as_ptr(&in_flight) as usize;

    let mut listeners = Vec::with_capacity(specs.len());
    for spec in specs {
        let kind = match spec.outcome {
            Ok(ListenerSpecKind::Plain) => ListenerKind::Plain,
            Ok(ListenerSpecKind::Tls(identity)) => match server_config(&identity) {
                Ok(config) => ListenerKind::Tls(config),
                Err(reason) => fail_at(
                    site,
                    codes::IDENTITY_LOAD_FAILED,
                    &format!("TLS identity for {}: {reason}", spec.address),
                    RUNTIME_EXIT_CODE,
                ),
            },
            Err(has_identity) => fail_at(
                site,
                codes::LISTENER_IDENTITY_MISMATCH_AT_STARTUP,
                &format!(
                    "listener {}: {}",
                    spec.address,
                    if has_identity {
                        "a Plain listener's identity must be None"
                    } else {
                        "a TLS listener needs an identity other than None"
                    }
                ),
                RUNTIME_EXIT_CODE,
            ),
        };
        match start_listener(
            &spec.address,
            kind,
            handler_fn,
            handler_env,
            &stopping,
            &in_flight,
            &open_connections,
            server_identity,
            handshake_timeout_seconds,
        ) {
            Ok(listener) => listeners.push(listener),
            Err(reason) => fail_at(
                site,
                codes::BIND_FAILED,
                &format!("core.net.@tcpServe: bind {}: {reason}", spec.address),
                RUNTIME_EXIT_CODE,
            ),
        }
    }

    let server_id = next_handle();
    with_worker(|worker| {
        worker
            .registry
            .servers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                server_id,
                Arc::new(ServerState {
                    stopping,
                    in_flight,
                    open_connections,
                    listeners,
                }),
            );
    });
    server_id as f64
}

/// Place an accepted connection on the worker with the shortest run queue right now: assign
/// its handle id and record it (and its raw descriptor) in the server's shared
/// `open_connections` map synchronously, on the accept loop's own worker, then place the
/// registration and handler call as a fiber on the chosen worker
/// (`crate::scheduler::spawn_placed`). For a `TLS` listener, that placed fiber first runs
/// the handshake — bounded by `handshake_timeout_seconds`, closing the connection silently
/// (same as a handshake rustls itself rejects) past it — before the handler ever sees it.
/// Auto-closes the connection once the handler returns, whether or not the handler closed
/// it itself, and only then leaves `in_flight`, so `Server.kill`'s wait sees this handler as
/// in-flight for its whole run, not just until its connection closes.
#[allow(clippy::too_many_arguments)]
fn place_connection(
    raw_stream: mio::net::TcpStream,
    kind: &ListenerKind,
    handler_fn: extern "C" fn(f64, *mut c_void) -> u8,
    handler_env: *mut c_void,
    in_flight: Arc<AtomicUsize>,
    open_connections: Arc<Mutex<HashMap<u64, RawFd>>>,
    server_identity: usize,
    handshake_timeout_seconds: f64,
) {
    let id = next_handle();
    let raw_fd = raw_stream.as_raw_fd();
    open_connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id, raw_fd);
    in_flight.fetch_add(1, Ordering::SeqCst);

    // `handler_env` isn't `Send`; it crosses to whichever worker ends up running the
    // handler as a `usize`, the same way `net::client`'s own pointers cross a `spawn`
    // boundary — cast back before use, on that worker's own thread.
    let handler_env_addr = handler_env as usize;
    let kind = kind.clone();
    crate::scheduler::spawn_placed(move || {
        let handler_env = handler_env_addr as *mut c_void;
        let stream = match TcpStream::from_accepted(raw_stream) {
            Ok(stream) => stream,
            Err(_) => {
                open_connections
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                in_flight.fetch_sub(1, Ordering::SeqCst);
                return;
            }
        };

        let transport = match kind {
            ListenerKind::Plain => ConnectionTransport::Plain(stream),
            ListenerKind::Tls(config) => {
                let mut stream = stream;
                let deadline = Instant::now() + bounded_duration(handshake_timeout_seconds);
                let Ok(connection) = handshake_server(config, &mut stream, deadline) else {
                    // A failed (or timed-out) handshake is closed silently, `stream`
                    // dropping here; the handler never runs.
                    open_connections
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&id);
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    return;
                };
                ConnectionTransport::Tls {
                    stream,
                    connection: Box::new(connection),
                }
            }
        };

        with_worker(|worker| {
            worker.connections.borrow_mut().insert(
                id,
                Rc::new(ConnectionState {
                    stream: RefCell::new(Some(transport)),
                    open_connections: Arc::clone(&open_connections),
                }),
            );
        });

        // Registered for the whole handler call so `Server.kill`, if this handler is the
        // one that calls it, can tell it is being asked to wait on its own fiber and
        // exclude it — see `wait_for_in_flight`.
        let fiber_id = current_fiber_id().expect("a spawned fiber has an id while it runs");
        with_worker(|worker| {
            worker
                .handler_fiber_server
                .borrow_mut()
                .insert(fiber_id, server_identity)
        });
        handler_fn(id as f64, handler_env);
        with_worker(|worker| worker.handler_fiber_server.borrow_mut().remove(&fiber_id));
        close_connection(id);
        in_flight.fetch_sub(1, Ordering::SeqCst);
    });
}

/// Close connection `id` if it is still open (idempotent: a no-op for one already closed) —
/// takes the stream out of its table entry, which drops and deregisters it, and removes
/// `id` from its server's open-connection set. Reused by `Connection.close()` and the
/// post-handler auto-close.
fn close_connection(id: u64) {
    let Some(state) = with_worker(|worker| worker.connections.borrow_mut().remove(&id)) else {
        return;
    };
    state
        .open_connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id);
    state.stream.borrow_mut().take();
}

/// Shut a connection down at the descriptor level, without touching the owning worker's
/// `connections` table at all — `Server.kill`'s force-close of a connection still open past
/// its grace period, callable from any worker, reading only the raw descriptor `kill`'s own
/// `ServerState::open_connections` map already carries. `shutdown(2)` needs no more: acting
/// on the descriptor directly is what lets it interrupt a handler fiber parked
/// mid-read/write on this same connection (on its own worker, unreachable from here) rather
/// than deadlock behind it.
///
/// ponytail: a permanently stuck handler (one that never touches this connection again
/// after the shutdown) leaves that drop — and its fiber's stack — unreclaimed forever. A
/// fiber-cancellation primitive would let this reclaim the stack instead of leaking it.
fn force_shutdown_connection(raw_fd: RawFd) {
    // SAFETY: `raw_fd` names a socket this process owns for as long as it stays in the
    // server's own `open_connections` map, which the caller (`__server_kill`) reads it from
    // and has not yet removed it from; `shutdown` only changes the socket's protocol state,
    // never its descriptor's validity.
    unsafe {
        libc::shutdown(raw_fd, libc::SHUT_RDWR);
    }
}

/// `Connection.@read()`: launch a background read of whatever bytes have arrived and
/// return the deferred `Text` immediately. `""` once the connection is closed (by the peer,
/// `Connection.close()`, or `Server.kill`) — a genuine read error reads the same way, since
/// `Text` carries no channel to report one.
#[unsafe(no_mangle)]
pub extern "C" fn __connection_read_launch(connection_id: f64) -> QnSlice {
    let id = connection_id as u64;
    launch_deferred_text_here(move || read_connection_once(id))
}

/// Read once from connection `id`'s stream, parking on readiness until data or EOF/an error
/// arrives. `""` when the connection has already closed, at EOF, or on any read error.
fn read_connection_once(id: u64) -> QnSlice {
    let Some(state) = with_worker(|worker| worker.connections.borrow().get(&id).cloned()) else {
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

/// `Connection.@read(seconds)`: like `__connection_read_launch`, but gives up and returns
/// `""` once `seconds` pass with nothing arriving — `core.http`'s idle-timeout overload.
#[unsafe(no_mangle)]
pub extern "C" fn __connection_read_with_timeout_launch(
    connection_id: f64,
    seconds: f64,
) -> QnSlice {
    let id = connection_id as u64;
    // `bounded_duration`, not a bare `Duration::from_secs_f64`: `seconds` is ordinary
    // Quilon arithmetic (`1.0 / 0.0` is infinity, not a language error), and that call
    // panics — taking the whole server down — on a value that is not finite.
    let deadline = Instant::now() + bounded_duration(seconds);
    launch_deferred_text_here(move || read_connection_once_with_deadline(id, deadline))
}

/// [`read_connection_once`]'s timed counterpart: `""` at EOF, on any read error, on a
/// missing/already-closed connection, OR once `deadline` passes with nothing arriving.
fn read_connection_once_with_deadline(id: u64, deadline: Instant) -> QnSlice {
    let Some(state) = with_worker(|worker| worker.connections.borrow().get(&id).cloned()) else {
        return alloc_text(&[]);
    };
    let mut slot = state.stream.borrow_mut();
    let Some(stream) = slot.as_mut() else {
        return alloc_text(&[]);
    };
    let mut buffer = [0u8; 4096];
    match stream.read_with_deadline(&mut buffer, deadline) {
        Ok(count) => alloc_text(&buffer[..count]),
        Err(_) => alloc_text(&[]),
    }
}

/// `Connection.@write(bytes)`: write every byte, parking on writability until all of it is
/// sent. Effect-only (`-> $`); a write past a closed connection is a silent no-op.
///
/// # Safety contract (upheld by the compiler)
/// `data` is null, or points to `len` readable bytes for the duration of this call.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __connection_write(connection_id: f64, data: *const u8, len: i64) {
    let id = connection_id as u64;
    let bytes = copy_bytes(data, len);
    let Some(state) = with_worker(|worker| worker.connections.borrow().get(&id).cloned()) else {
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

/// Wake every one of a server's accept loops out of a parked `accept()` with nothing
/// pending, so each notices `stopping` promptly: connect to the listener's own bound port
/// from loopback, in the same address family it bound — connecting in the wrong family
/// fails outright, leaving that accept loop parked forever with no genuine client to wake
/// it either, which is exactly the hang this match avoids. Best-effort otherwise; either way
/// `kill` still [`settle`]s every accept loop before returning.
fn wake_accept_loop(server: &ServerState) {
    for listener in &server.listeners {
        let loopback = match listener.local_addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        };
        let target = SocketAddr::new(loopback, listener.local_addr.port());
        let _ = TcpStream::connect(target);
    }
}

/// The largest deadline this module honors for a caller-supplied `seconds` — `Server.kill`'s
/// grace period, `Connection.@read`'s timeout, and a `TLS` listener's handshake timeout.
/// `Duration::from_secs_f64` panics on a value that is not finite or overflows `Duration`
/// (and `seconds` is ordinary Quilon arithmetic, so `1.0 / 0.0` is infinity, not a language
/// error) — clamping into this range before ever calling it keeps a wild `seconds` from
/// taking the whole process down with it.
const MAX_DEADLINE_SECONDS: f64 = 1_000_000_000.0;

/// `seconds` as a `Duration`, never panicking: NaN and a negative value both become "no
/// wait", and an infinite or overflowing value is capped at [`MAX_DEADLINE_SECONDS`].
fn bounded_duration(seconds: f64) -> Duration {
    let bounded = if seconds.is_nan() {
        0.0
    } else if !seconds.is_finite() {
        if seconds.is_sign_positive() {
            MAX_DEADLINE_SECONDS
        } else {
            0.0
        }
    } else {
        seconds.clamp(0.0, MAX_DEADLINE_SECONDS)
    };
    Duration::from_secs_f64(bounded)
}

/// Park the calling fiber, in short sleeps, until `server`'s `in_flight` count reaches zero
/// — or, when the calling fiber is itself one of `server`'s own handlers (a handler answers
/// by calling `server.kill()`), until it reaches one, since that one is this handler's own
/// count and it cannot leave `in_flight` until `kill` does. Without this exclusion, a `kill`
/// called from inside a handler always burns its whole grace period, every time.
///
/// ponytail: a fixed-tick poll rather than a wake on the last handler's own finish. Upgrade
/// to a cross-worker wait/wake pair (`crate::deferred::Waiters`) keyed by the server if that
/// latency ever matters.
fn wait_for_in_flight(server: &ServerState, seconds: f64) {
    const TICK: Duration = Duration::from_millis(20);
    let identity = Arc::as_ptr(&server.in_flight) as usize;
    let calling_fiber_is_this_servers_own_handler = current_fiber_id().and_then(|fiber_id| {
        with_worker(|worker| worker.handler_fiber_server.borrow().get(&fiber_id).copied())
    }) == Some(identity);
    let floor = usize::from(calling_fiber_is_this_servers_own_handler);
    let deadline = Instant::now() + bounded_duration(seconds);
    while server.in_flight.load(Ordering::SeqCst) > floor && Instant::now() < deadline {
        sleep(TICK);
    }
}

/// `Server.kill(seconds)`: stop accepting, wait up to `seconds` for in-flight handlers to
/// finish — on every worker one may be running on — force-close any connection still open
/// past that grace period, then settle every listener's accept loop so the enclosing
/// block's join finds them already done. Callable from any worker; a no-op on an
/// already-killed or unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn __server_kill(server_id: f64, seconds: f64) {
    let id = server_id as u64;
    let Some(server) = with_worker(|worker| {
        worker
            .registry
            .servers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id)
    }) else {
        return;
    };
    server.stopping.store(true, Ordering::SeqCst);
    wake_accept_loop(&server);
    wait_for_in_flight(&server, seconds);
    let stuck: Vec<RawFd> = server
        .open_connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .copied()
        .collect();
    for raw_fd in stuck {
        force_shutdown_connection(raw_fd);
    }
    for listener in &server.listeners {
        // SAFETY: `accept_loop` is the cell `launch_here` returned for this listener's own
        // accept loop, still reachable through `server` (this function's only owner of it,
        // since the handle was just removed from the registry's `servers` table above).
        unsafe {
            settle(listener.accept_loop);
        }
    }
}

/// `server_id`'s listeners' bound `SocketAddr`s, in list order, or `None` on an
/// already-killed or unknown handle. Callable from any worker.
fn bound_addresses(server_id: f64) -> Option<Vec<SocketAddr>> {
    let id = server_id as u64;
    with_worker(|worker| {
        worker
            .registry
            .servers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .map(|server| server.listeners.iter().map(|l| l.local_addr).collect())
    })
}

/// `Server.address()`'s fatal guard: a server with other than exactly one listener has no
/// single address for it to report. A no-op on an already-killed or unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_guard(server_id: f64, site: *const QnSite) {
    let Some(addresses) = bound_addresses(server_id) else {
        return;
    };
    if addresses.len() != 1 {
        fail_at(
            site,
            codes::AMBIGUOUS_SERVER_ADDRESS,
            &format!(
                "this server has {} listeners; use addresses()",
                addresses.len()
            ),
            RUNTIME_EXIT_CODE,
        );
    }
}

/// `Server.address()`'s `host` half, rendered bare (`Address.text()` adds brackets) — the
/// first (and, past [`__server_address_guard`], only) listener's.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_host(server_id: f64) -> QnSlice {
    bound_addresses(server_id)
        .and_then(|addresses| addresses.into_iter().next())
        .map_or_else(
            || alloc_text(&[]),
            |a| alloc_text(a.ip().to_string().as_bytes()),
        )
}

/// `Server.address()`'s `port` half.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_port(server_id: f64) -> f64 {
    bound_addresses(server_id)
        .and_then(|addresses| addresses.into_iter().next())
        .map_or(0.0, |a| f64::from(a.port()))
}

/// `Server.addresses()`'s listener count, so codegen can build the `[]Address` array
/// before filling it — [`__server_address_host_at`]/[`__server_address_port_at`] read one
/// listener's bound address at a time, by index in list order.
#[unsafe(no_mangle)]
pub extern "C" fn __server_listener_count(server_id: f64) -> f64 {
    bound_addresses(server_id).map_or(0.0, |addresses| addresses.len() as f64)
}

/// `Server.addresses()`'s `host` half for the listener at `index` (0-based, list order).
/// `""` past the end, or on an already-killed or unknown handle — codegen only ever asks
/// for an index under [`__server_listener_count`]'s own answer.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_host_at(server_id: f64, index: f64) -> QnSlice {
    bound_addresses(server_id)
        .and_then(|addresses| addresses.into_iter().nth(index as usize))
        .map_or_else(
            || alloc_text(&[]),
            |a| alloc_text(a.ip().to_string().as_bytes()),
        )
}

/// `Server.addresses()`'s `port` half for the listener at `index`. `0` past the end, or on
/// an already-killed or unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_port_at(server_id: f64, index: f64) -> f64 {
    bound_addresses(server_id)
        .and_then(|addresses| addresses.into_iter().nth(index as usize))
        .map_or(0.0, |a| f64::from(a.port()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::__force_text;
    use crate::gc_test_harness::on_gc_thread;
    use crate::scheduler::{run, sleep, spawn};
    use std::io::{Read, Write};
    use std::os::raw::c_void;
    use std::ptr;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Where `net.@tcpServe` actually bound `server_id` — a whitebox peek at the private
    /// table, standing in for the ephemeral port a test client needs.
    fn bound_addr(server_id: f64) -> SocketAddr {
        with_worker(|worker| {
            worker
                .registry
                .servers()
                .lock()
                .unwrap()
                .get(&(server_id as u64))
                .expect("server handle")
                .listeners[0]
                .local_addr
        })
    }

    /// `__tcp_serve_launch`'s `address_data`/`address_len` are a `Text`'s own raw fields —
    /// header included, exactly like `write_connection_bytes`'s below — so every test binds
    /// through this helper rather than building that pair by hand at each call site. The
    /// 5-second handshake timeout matches `ServerOptions.default()`'s own, though no test
    /// here ever runs a `TLS` listener.
    fn launch_test_server(address: &str, handler_fn: extern "C" fn(f64, *mut c_void) -> u8) -> f64 {
        let (address_ptr, address_len) = crate::test_support::text_of(address);
        __tcp_serve_launch(
            address_ptr,
            address_len,
            handler_fn as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            5.0,
        )
    }

    /// Force `__connection_read_launch`'s deferred `Text` the way generated code does:
    /// extract the promise from its sentinel-tagged slice and force it.
    fn force_connection_read(connection_id: f64) -> Vec<u8> {
        let deferred = __connection_read_launch(connection_id);
        let forced = __force_text(deferred.data);
        crate::text::byte_slice(forced.data as *const u8, forced.len).to_vec()
    }

    /// [`force_connection_read`]'s timed counterpart, forcing
    /// `__connection_read_with_timeout_launch`'s deferred `Text` the same way.
    fn force_connection_read_with_timeout(connection_id: f64, seconds: f64) -> Vec<u8> {
        let deferred = __connection_read_with_timeout_launch(connection_id, seconds);
        let forced = __force_text(deferred.data);
        crate::text::byte_slice(forced.data as *const u8, forced.len).to_vec()
    }

    /// `__connection_write`'s `data`/`len` are a `Text`'s own raw fields — header included,
    /// as codegen extracts them from a real `bytes :: Text` argument — so a test calling it
    /// directly builds one the same way `text_of_bytes` builds every other low-level `Text`
    /// fixture in this crate's tests, rather than handing it a bare buffer's pointer.
    fn write_connection_bytes(connection_id: f64, bytes: &[u8]) {
        let (ptr, len) = crate::test_support::text_of_bytes(bytes);
        __connection_write(connection_id, ptr, len);
    }

    /// A std client with a bounded read/write timeout on every socket op it does — every
    /// test below connects this way, so a bug that leaves a connection open with nothing
    /// arriving fails the test in a few seconds instead of hanging the run.
    fn connect_with_timeout(addr: SocketAddr) -> std::net::TcpStream {
        let stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .expect("connect to the test server");
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
        // connection to its own fiber, running two clients' exchanges concurrently rather
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
                    let server_id = launch_test_server("127.0.0.1:0", echo_handler);
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
                    let server_id = launch_test_server("127.0.0.1:0", forgetful_handler);
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
                    let server_id = launch_test_server("127.0.0.1:0", slow_echo_handler);
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
                    let server_id = launch_test_server("127.0.0.1:0", stuck_handler);
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

    #[test]
    fn kill_called_from_its_own_handler_does_not_wait_out_the_whole_grace_period() {
        // A handler that answers by calling `server.kill(seconds)` on its own server is
        // itself still "in flight" until that call returns, so without excluding the
        // calling fiber from its own wait, `kill` would always burn its whole grace
        // period. Proven by timing the whole run: an unfixed wait would take (at least)
        // the 5-second grace period below; the fix returns almost at once.
        static SERVER_HANDLE: StdMutex<f64> = StdMutex::new(0.0);

        extern "C" fn self_killing_handler(_connection_id: f64, _environment: *mut c_void) -> u8 {
            let server_id = *SERVER_HANDLE.lock().unwrap();
            __server_kill(server_id, 5.0);
            0
        }

        let start = Instant::now();
        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = launch_test_server("127.0.0.1:0", self_killing_handler);
                    *SERVER_HANDLE.lock().unwrap() = server_id;
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let _ = connect_with_timeout(addr);
                    });
                    client.join().expect("client thread panicked");
                });
            });
        });

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "kill(5) called from inside its own handler must not wait out its grace \
             period, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn accept_and_echo_over_ipv6_loopback() {
        // Skips, rather than fails, on a machine with no IPv6 loopback configured: binding
        // `[::1]:0` there fails with "address not available", an environment limitation
        // this test cannot fix, and the runtime's own bind failure is fatal — so this
        // probes with a throwaway std bind first, never reaching `__tcp_serve_launch` at
        // all if that probe fails.
        if std::net::TcpListener::bind("[::1]:0").is_err() {
            eprintln!(
                "skipping accept_and_echo_over_ipv6_loopback: no IPv6 loopback on this machine"
            );
            return;
        }

        extern "C" fn echo_handler(connection_id: f64, _environment: *mut c_void) -> u8 {
            let bytes = force_connection_read(connection_id);
            write_connection_bytes(connection_id, &bytes);
            0
        }

        static FINISHED: AtomicBool = AtomicBool::new(false);
        FINISHED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = launch_test_server("[::1]:0", echo_handler);
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        stream
                            .write_all(b"hexagon")
                            .expect("write the echo message");
                        let mut echoed = [0u8; 7];
                        stream.read_exact(&mut echoed).expect("read the echo");
                        assert_eq!(&echoed, b"hexagon", "the echoed bytes matched");
                        FINISHED.store(true, Ordering::SeqCst);
                    });

                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !FINISHED.load(Ordering::SeqCst) && Instant::now() < deadline {
                        sleep(Duration::from_millis(5));
                    }
                    client.join().expect("client thread panicked");
                    __server_kill(server_id, 1.0);
                });
            });
        });

        assert!(FINISHED.load(Ordering::SeqCst), "the IPv6 echo completed");
    }

    #[test]
    fn address_after_binding_port_zero_reports_the_listeners_own_port() {
        // `__server_address_port` must read back what the OS actually bound, not the `0`
        // the program asked for — and `__server_address_host` must agree with the same
        // `local_addr` on the host half.
        extern "C" fn unreached_handler(_connection_id: f64, _environment: *mut c_void) -> u8 {
            0
        }

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = launch_test_server("127.0.0.1:0", unreached_handler);
                    let bound = bound_addr(server_id);

                    assert_ne!(bound.port(), 0, "the listener itself bound a real port");
                    assert_eq!(
                        __server_address_port(server_id),
                        f64::from(bound.port()),
                        "address() must report the listener's own port"
                    );
                    let host = __server_address_host(server_id);
                    let host = crate::text::byte_slice(host.data as *const u8, host.len);
                    assert_eq!(
                        host,
                        bound.ip().to_string().as_bytes(),
                        "address() must report the listener's own host"
                    );

                    __server_kill(server_id, 1.0);
                });
            });
        });
    }

    #[test]
    fn read_with_timeout_returns_data_that_arrives_before_the_deadline() {
        // A generous deadline and a prompt client: the read returns the real bytes, not a
        // timeout — the common case, where the timeout never actually fires.
        extern "C" fn echo_with_generous_timeout(
            connection_id: f64,
            _environment: *mut c_void,
        ) -> u8 {
            let bytes = force_connection_read_with_timeout(connection_id, 5.0);
            write_connection_bytes(connection_id, &bytes);
            0
        }

        static FINISHED: AtomicBool = AtomicBool::new(false);
        FINISHED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = launch_test_server("127.0.0.1:0", echo_with_generous_timeout);
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        stream.write_all(b"sundial").expect("write the message");
                        let mut echoed = [0u8; 7];
                        stream.read_exact(&mut echoed).expect("read the echo");
                        assert_eq!(&echoed, b"sundial");
                        FINISHED.store(true, Ordering::SeqCst);
                    });

                    // Yield cooperatively (`sleep`, not `client.join()` outright) until the
                    // client is done — a direct join would block this OS thread, the only
                    // one driving the scheduler, before the handler fiber ever gets to run.
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !FINISHED.load(Ordering::SeqCst) && Instant::now() < deadline {
                        sleep(Duration::from_millis(5));
                    }
                    client.join().expect("client thread panicked");
                    __server_kill(server_id, 1.0);
                });
            });
        });

        assert!(FINISHED.load(Ordering::SeqCst), "the timed echo completed");
    }

    #[test]
    fn read_with_timeout_returns_empty_once_the_deadline_passes_with_nothing_arriving() {
        // A client that connects and then never writes: the handler's timed read gives up
        // on its own, echoing back an empty reply rather than the connection hanging until
        // `kill` force-closes it.
        extern "C" fn echo_with_short_timeout(connection_id: f64, _environment: *mut c_void) -> u8 {
            let bytes = force_connection_read_with_timeout(connection_id, 0.05);
            write_connection_bytes(connection_id, &bytes);
            0
        }

        static FINISHED: AtomicBool = AtomicBool::new(false);
        FINISHED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let server_id = launch_test_server("127.0.0.1:0", echo_with_short_timeout);
                    let addr = bound_addr(server_id);

                    let client = std::thread::spawn(move || {
                        let mut stream = connect_with_timeout(addr);
                        // Never writes: the timeout, not a real message, must end the read.
                        let mut echoed = [0u8; 1];
                        let outcome = stream.read(&mut echoed);
                        // The timed-out handler writes zero bytes and returns, and the
                        // runtime's own auto-close then ends the connection — read as EOF.
                        assert!(matches!(outcome, Ok(0)), "expected EOF, got {outcome:?}");
                        FINISHED.store(true, Ordering::SeqCst);
                    });

                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !FINISHED.load(Ordering::SeqCst) && Instant::now() < deadline {
                        sleep(Duration::from_millis(5));
                    }
                    client.join().expect("client thread panicked");
                    __server_kill(server_id, 1.0);
                });
            });
        });

        assert!(
            FINISHED.load(Ordering::SeqCst),
            "the timed-out read closed the connection"
        );
    }

    #[test]
    fn bounded_duration_never_panics_on_a_non_finite_or_absurd_seconds() {
        // `seconds` is ordinary Quilon Num arithmetic — `1.0 / 0.0` is infinity, not a
        // language error — so none of these may reach `Duration::from_secs_f64`'s own
        // panic conditions. Shared by `Server.kill`'s grace period, `Connection.@read`'s
        // timeout, and a TLS handshake's own, so one test covers all three callers.
        for seconds in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            f64::MAX,
            0.0,
            5.0,
        ] {
            let _ = bounded_duration(seconds);
        }
    }
}
