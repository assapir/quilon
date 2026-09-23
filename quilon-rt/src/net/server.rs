// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! `net.@tcpServe`: the raw TCP server layer.
//!
//! The runtime owns only what the language cannot express: accepting connections and
//! running each on its own fiber. `Connection`/`Server` are opaque handles — a `Num` id
//! into the thread-local tables below — with every method compiler-lowered (see
//! `src/codegen/generator/calls.rs`'s `generate_at_primitive` and its `close`/`kill`
//! interception ahead of ordinary method dispatch).
//!
//! `net.@tcpServe`'s accept loop is a launch registered directly with `launch_scope`
//! (through the generic `deferred::launch`, whose own return value — the deferred cell
//! pointer — is kept on `ServerState` rather than exposed to Quilon, since `@tcpServe`'s
//! OWN return value, the `Server` handle, is a ready `Num` built by codegen, never
//! deferred). `Server.kill` settles that same cell before returning, so the enclosing
//! block's own join finds it already done. Built on `super::TcpListener` and
//! [`super::TcpStream`], the plumbing shared with [`super::client`].

use super::{TcpListener, TcpStream, bytes_to_string, copy_bytes, resolve};
use crate::deferred::{launch, launch_deferred_text, settle};
use crate::mem::{QlSlice, alloc_text};
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
use crate::scheduler::{current_fiber_id, sleep, spawn};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::raw::c_void;
use std::os::unix::io::RawFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
    /// Which server's `in_flight` count a currently-running handler fiber counts against,
    /// keyed by fiber id — see [`wait_for_in_flight`].
    static HANDLER_FIBER_SERVER: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
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

/// `net.@tcpServe(address, handler)`: resolve `address` exactly as `resolve` resolves
/// `@tcpRequest`'s own (a numeric IPv4/IPv6 address, an IPv6 literal in brackets, or a
/// hostname resolved on the runtime's blocking-call pool — parking the calling fiber, not
/// the accept loop, until it answers), bind and listen on it, and return the `Server`
/// handle's id at once — codegen builds the `Server { handle = … }` record around it, the
/// same way it builds the `Connection` handed to `handler`. The accept loop launches on a
/// background fiber, registered with whatever `< >` block's launch scope is open right now
/// (through `launch`, exactly as a value-returning primitive's producer registers) so
/// that block's own join keeps it alive without ever forcing a value from this call — this
/// call's own return is already ready. A bind failure — `address` does not parse or
/// resolve, the port is missing or out of range, the port is already in use, or
/// permission is refused — is fatal, reported at `site` through the same fail-loud path
/// every other unrecoverable runtime check uses, naming `address` as written.
///
/// # Safety contract (upheld by the compiler)
/// `address_data` is null, or points to `address_len` readable bytes for the duration of
/// this call (a `Text`'s live bytes at the call site); `handler_fn` is a live `(f64, ptr)
/// -> i8` trampoline — codegen's fixed-shape wrapper (see
/// `CodeGenerator::emit_tcp_serve_handler_thunk`) over the user's `(Connection) -> $`
/// closure — called with `handler_env` as its second argument; `site` is null or points to
/// a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __tcp_serve_launch(
    address_data: *const u8,
    address_len: i64,
    handler_fn: *const c_void,
    handler_env: *mut c_void,
    site: *const QlSite,
) -> f64 {
    let address = bytes_to_string(address_data, address_len);
    // SAFETY: per the contract, a live `(f64, ptr) -> i8` trampoline.
    let handler_fn: extern "C" fn(f64, *mut c_void) -> u8 =
        unsafe { std::mem::transmute(handler_fn) };

    let bind = |reason: String| -> ! {
        fail_at(
            site,
            codes::BIND_FAILED,
            &format!("core.net.@tcpServe: bind {address}: {reason}"),
            RUNTIME_EXIT_CODE,
        )
    };
    let bind_addr = match resolve(&address) {
        Ok(addr) => addr,
        Err(error) => bind(error.to_string()),
    };
    let mut listener = match TcpListener::bind(bind_addr) {
        Ok(listener) => listener,
        Err(error) => bind(error.to_string()),
    };
    let local_addr = listener.local_addr().unwrap_or(bind_addr);

    let stopping = Rc::new(Cell::new(false));
    let in_flight = Rc::new(Cell::new(0usize));
    let open_connections: Rc<RefCell<HashSet<u64>>> = Rc::new(RefCell::new(HashSet::new()));
    // This server's identity for `HANDLER_FIBER_SERVER` — the `in_flight` cell's own
    // address, stable for as long as any clone of it (every one taken below, and the
    // `ServerState` itself) is alive.
    let server_identity = Rc::as_ptr(&in_flight) as usize;

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
                server_identity,
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
    server_identity: usize,
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
        // Registered for the whole handler call so `Server.kill`, if THIS handler is the
        // one that calls it, can tell it is being asked to wait on its own fiber and
        // exclude it — see `wait_for_in_flight`.
        let fiber_id = current_fiber_id().expect("a spawned fiber has an id while it runs");
        HANDLER_FIBER_SERVER
            .with(|handlers| handlers.borrow_mut().insert(fiber_id, server_identity));
        handler_fn(id as f64, handler_env);
        HANDLER_FIBER_SERVER.with(|handlers| handlers.borrow_mut().remove(&fiber_id));
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
/// come): connect to the listener's own bound port from loopback, in the SAME address
/// family it bound (`127.0.0.1` for an IPv4 listener, `::1` for an IPv6 one) — connecting
/// in the wrong family fails outright (nothing is listening there), leaving the accept
/// loop parked forever with no genuine client to wake it either, which is exactly the hang
/// this match avoids. Best-effort otherwise (a listener bound to one specific non-loopback
/// interface address is not reachable via loopback at all) — a failure here just means a
/// genuine client connection (or `Server.kill`'s own grace period ending) wakes it instead;
/// either way `kill` still [`settle`]s the accept loop before returning.
fn wake_accept_loop(server: &ServerState) {
    let loopback = match server.local_addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let target = SocketAddr::new(loopback, server.local_addr.port());
    let _ = TcpStream::connect(target);
}

/// The largest grace period `Server.kill` honors. Far longer than any reasonable use, but
/// a concrete bound: `seconds` is ordinary Quilon arithmetic (`1.0 / 0.0` is infinity, not
/// a language error), and `Duration::from_secs_f64` panics on a value that is not finite
/// or overflows `Duration` — clamping into this range before ever calling it keeps a wild
/// `seconds` from taking the whole process down with it.
const MAX_KILL_SECONDS: f64 = 1_000_000_000.0;

/// `seconds` as a `Duration`, never panicking: NaN and a negative value both become "no
/// wait", and an infinite or overflowing value is capped at [`MAX_KILL_SECONDS`].
fn kill_grace_period(seconds: f64) -> Duration {
    let bounded = if seconds.is_nan() {
        0.0
    } else if !seconds.is_finite() {
        if seconds.is_sign_positive() {
            MAX_KILL_SECONDS
        } else {
            0.0
        }
    } else {
        seconds.clamp(0.0, MAX_KILL_SECONDS)
    };
    Duration::from_secs_f64(bounded)
}

/// Park the calling fiber, in short sleeps, until `server`'s `in_flight` count reaches zero
/// — or, when the CALLING fiber is itself one of `server`'s own handlers (the deliverable's
/// own pattern: a handler answers by calling `server.kill()`), until it reaches one, since
/// that one is this handler's own count and it cannot leave `in_flight` — this call, and so
/// the handler itself, has not returned yet — until `kill` does. Without this exclusion, a
/// `kill` called from inside a handler always burns its whole grace period, every time.
///
/// ponytail: a fixed-tick poll rather than a wake on the last handler's own finish — the
/// simplest correct wait, at the cost of up to one tick of latency past the last handler
/// actually finishing. Upgrade to a `park_on_address`/`wake_address` pair keyed by the
/// server if that latency ever matters.
fn wait_for_in_flight(server: &ServerState, seconds: f64) {
    const TICK: Duration = Duration::from_millis(20);
    let identity = Rc::as_ptr(&server.in_flight) as usize;
    let calling_fiber_is_this_servers_own_handler = current_fiber_id().and_then(|fiber_id| {
        HANDLER_FIBER_SERVER.with(|handlers| handlers.borrow().get(&fiber_id).copied())
    }) == Some(identity);
    let floor = usize::from(calling_fiber_is_this_servers_own_handler);
    let deadline = Instant::now() + kill_grace_period(seconds);
    while server.in_flight.get() > floor && Instant::now() < deadline {
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

/// `Server.address()`'s `host` half: the bound `SocketAddr`'s IP, rendered bare (never
/// bracketed — `Address.text()`, the Quilon side, is what adds brackets for an IPv6 host).
/// `""` on an already-killed or unknown handle, the same missing-entry policy
/// `Connection.@read()` uses.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_host(server_id: f64) -> QlSlice {
    let id = server_id as u64;
    let Some(server) = SERVERS.with(|servers| servers.borrow().get(&id).cloned()) else {
        return alloc_text(&[]);
    };
    alloc_text(server.local_addr.ip().to_string().as_bytes())
}

/// `Server.address()`'s `port` half: the bound `SocketAddr`'s port — never `0`, even when
/// `@tcpServe` was asked for one, since this reads back what the OS actually bound. `0` on
/// an already-killed or unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn __server_address_port(server_id: f64) -> f64 {
    let id = server_id as u64;
    SERVERS
        .with(|servers| servers.borrow().get(&id).cloned())
        .map_or(0.0, |server| f64::from(server.local_addr.port()))
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

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

    /// `__tcp_serve_launch`'s `address_data`/`address_len` are a `Text`'s own raw fields —
    /// header included, exactly like `write_connection_bytes`'s below — so every test binds
    /// through this helper rather than building that pair by hand at each call site.
    fn launch_test_server(address: &str, handler_fn: extern "C" fn(f64, *mut c_void) -> u8) -> f64 {
        let (address_ptr, address_len) = crate::test_support::text_of(address);
        __tcp_serve_launch(
            address_ptr,
            address_len,
            handler_fn as *const c_void,
            ptr::null_mut(),
            ptr::null(),
        )
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
        // A bounded `connect_timeout`, not a plain `connect`: an address a listener is
        // genuinely refusing fails at once, but one silently dropped (a firewalled or
        // otherwise unreachable address, IPv6 loopback being the one this crate's own
        // tests have hit) never fails on its own — only a deadline does.
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
        // A handler that answers by calling `server.kill(seconds)` on its OWN server — the
        // deliverable's own pattern — is itself still "in flight" until that call returns,
        // so without excluding the calling fiber from its own wait, `kill` would always
        // burn its whole grace period. Proven by timing the whole run: an unfixed wait
        // would take (at least) the 5-second grace period below; the fix returns almost
        // at once, since nothing else is in flight.
        static SERVER_HANDLE: Mutex<f64> = Mutex::new(0.0);

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
        // `net.@tcpServe`'s address argument is `host:port` exactly as `@tcpRequest` takes
        // it, including a numeric IPv6 literal in brackets. Skips, rather than fails, on a
        // machine with no IPv6 loopback configured: binding `[::1]:0` there fails with
        // "address not available", an environment limitation this test cannot fix, and the
        // runtime's own bind failure is fatal (exits the whole process) — not something a
        // test could catch and continue past — so this probes with a throwaway std bind
        // first, never reaching `__tcp_serve_launch` at all if that probe fails.
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
    fn kill_grace_period_never_panics_on_a_non_finite_or_absurd_seconds() {
        // `seconds` is ordinary Quilon Num arithmetic — `1.0 / 0.0` is infinity, not a
        // language error — so none of these may reach `Duration::from_secs_f64`'s own
        // panic conditions (negative, not finite, or overflowing `Duration`).
        for seconds in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            f64::MAX,
            0.0,
            5.0,
        ] {
            let _ = kill_grace_period(seconds);
        }
    }
}
