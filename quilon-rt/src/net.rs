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

use crate::deferred::{QlResult, launch_deferred_result};
use crate::reactor::ReactorWaker;
use crate::scheduler::{
    deregister_readiness, park_on_readiness, register_helper_waker, register_readiness,
    reregister_readiness,
};
use mio::event::Source;
use mio::{Interest, Token};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

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

/// A hostname lookup, boxed so [`ResolverJob`] can carry either the real DNS lookup or a
/// test double through the same channel.
type Lookup = Box<dyn FnOnce(&str) -> io::Result<SocketAddr> + Send>;

/// One hostname lookup for a [`ResolverPool`] worker to run: the address, the lookup itself
/// (the real DNS lookup in production, a test double in tests), and where to deliver the
/// outcome — the result channel back to the parked fiber, and the reactor token/waker that
/// wakes it once the result is sent.
struct ResolverJob {
    address: String,
    lookup: Lookup,
    result_sender: mpsc::Sender<io::Result<SocketAddr>>,
    waker: ReactorWaker,
    token: Token,
}

/// Lower bound on [`ResolverPool`]'s base thread count, regardless of CPU count — a
/// single-core machine still gets a few concurrent hostname lookups going.
const RESOLVER_POOL_BASE_MINIMUM: usize = 4;

/// Upper bound on [`ResolverPool`]'s total thread count (base threads plus every thread it
/// grows to under load). Not configurable — named here, never read from the environment — so
/// a lookup that arrives once the pool is already this large waits in the queue instead of
/// growing it further.
const RESOLVER_POOL_CEILING: usize = 16;

/// `getaddrinfo` has no non-blocking form and blocks for as long as the network takes, so
/// [`ResolverPool`] runs it on OS threads rather than the reactor thread: a small set of BASE
/// threads, started lazily on the first hostname lookup and kept for the rest of the process
/// (reused across lookups — a program that never resolves a name starts none), plus OVERFLOW
/// threads the pool grows by one at a time, up to [`RESOLVER_POOL_CEILING`] threads in total,
/// whenever a lookup arrives with more jobs in flight than the pool currently has threads for.
/// An overflow thread is temporary: it exits as soon as it finds the job queue empty, so the
/// pool shrinks back to its base once a burst of concurrent lookups drains.
struct ResolverPool {
    queue: JobQueue,
    state: Mutex<PoolState>,
}

/// The pool's shared job queue: a plain `VecDeque` behind a `Mutex`, with a `Condvar` a BASE
/// worker's blocking wait ([`pop_blocking`](Self::pop_blocking)) parks on. Not an
/// `mpsc::Receiver` shared behind a `Mutex`: `Receiver::recv` blocks WHILE holding whatever
/// lock guards it, so with more than one blocking consumer the first to call it would starve
/// every other base thread — and every overflow thread's non-blocking check — out of the
/// queue for good. `Condvar::wait` releases the mutex while parked, which is exactly what
/// avoids that.
struct JobQueue {
    jobs: Mutex<VecDeque<ResolverJob>>,
    condvar: Condvar,
}

impl JobQueue {
    fn new() -> Self {
        JobQueue {
            jobs: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        }
    }

    fn push(&self, job: ResolverJob) {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(job);
        // Exactly one job arrived, so waking one waiter (rather than every base thread) is
        // enough for it to be picked up.
        self.condvar.notify_one();
    }

    /// A base worker's wait: blocks until a job is available. `Condvar::wait` releases `jobs`'
    /// lock for the whole time this thread is parked, so other threads keep working the queue.
    fn pop_blocking(&self) -> ResolverJob {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(job) = jobs.pop_front() {
                return job;
            }
            jobs = self
                .condvar
                .wait(jobs)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// An overflow worker's check: `None` if the queue is empty right now, with no wait.
    fn pop_nonblocking(&self) -> Option<ResolverJob> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }
}

/// [`ResolverPool`]'s bookkeeping for its own growth decisions: how many threads it has right
/// now, and how many jobs are in flight (queued or running) — comparing the two, at every
/// submission, is what decides whether to grow (see [`submit_job`]).
struct PoolState {
    total_threads: usize,
    in_flight: usize,
}

/// The resolver pool's base thread count: the process's CPU count — honouring cgroup quotas
/// and affinity masks, via [`std::thread::available_parallelism`] — clamped to
/// [`RESOLVER_POOL_BASE_MINIMUM`]..=[`RESOLVER_POOL_CEILING`].
fn resolver_pool_base_size() -> usize {
    let cpu_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    cpu_count.clamp(RESOLVER_POOL_BASE_MINIMUM, RESOLVER_POOL_CEILING)
}

/// The lazily started global [`ResolverPool`]: absent until the first hostname lookup. A
/// `Result` cached here (not a bare pool) so a base-thread spawn failure the first time (thread
/// creation refused) is remembered and answered the same way on every later lookup — as an
/// ordinary `Err`/`NotOk`, never the process-aborting panic `thread::spawn` itself raises.
fn resolver_pool() -> io::Result<Arc<ResolverPool>> {
    static POOL: OnceLock<io::Result<Arc<ResolverPool>>> = OnceLock::new();
    match POOL.get_or_init(|| start_resolver_pool(resolver_pool_base_size())) {
        Ok(pool) => Ok(Arc::clone(pool)),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

/// Start a [`ResolverPool`] with `base_size` base worker threads, or the first thread-creation
/// failure. Split out from [`resolver_pool`] so a test can force a small base size rather than
/// the real CPU count.
fn start_resolver_pool(base_size: usize) -> io::Result<Arc<ResolverPool>> {
    let pool = Arc::new(ResolverPool {
        queue: JobQueue::new(),
        state: Mutex::new(PoolState {
            total_threads: 0,
            in_flight: 0,
        }),
    });
    for _ in 0..base_size {
        spawn_base_worker(&pool)?;
    }
    Ok(pool)
}

/// Spawn one base worker (a permanent thread, looping on [`run_base_worker`] for the pool's
/// whole life) and count it, or propagate the thread-creation failure without counting it.
fn spawn_base_worker(pool: &Arc<ResolverPool>) -> io::Result<()> {
    let worker_pool = Arc::clone(pool);
    std::thread::Builder::new().spawn(move || run_base_worker(&worker_pool))?;
    pool.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .total_threads += 1;
    Ok(())
}

/// A base worker's whole life: block for the next job and run it, forever — a base thread
/// never exits.
fn run_base_worker(pool: &ResolverPool) {
    loop {
        let job = pool.queue.pop_blocking();
        run_resolver_job(pool, job);
    }
}

/// An overflow worker's whole life: pull jobs without blocking, and exit the moment it finds
/// none waiting — this is what shrinks the pool back to its base once a burst of concurrent
/// lookups drains, rather than leaving idle threads running for the rest of the process.
fn run_overflow_worker(pool: &ResolverPool) {
    while let Some(job) = pool.queue.pop_nonblocking() {
        run_resolver_job(pool, job);
    }
    pool.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .total_threads -= 1;
}

/// Run one job's lookup and deliver its outcome, then mark it no longer in flight. Shared by
/// base and overflow workers alike — the only difference between them is how each one waits
/// for its next job, not what it does once it has one.
fn run_resolver_job(pool: &ResolverPool, job: ResolverJob) {
    // Caught rather than left to unwind off the end of the thread: an uncaught panic here
    // would never send a result or wake the reactor, leaving the parked fiber stuck forever —
    // worse than the failure `@tcpRequest` otherwise always turns into a `NotOk`.
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.lookup)(&job.address)))
            .unwrap_or_else(|_| Err(io::Error::other("the hostname resolver panicked")));
    let _ = job.result_sender.send(result);
    job.waker.complete(job.token);
    pool.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .in_flight -= 1;
}

/// Queue `job` on `pool`, growing the pool by one overflow thread ([`run_overflow_worker`]) if
/// more jobs are now in flight than the pool has threads for — the pigeonhole case: some job,
/// this one or an earlier one, has no thread that could possibly be running it yet. Never grows
/// past [`RESOLVER_POOL_CEILING`]; beyond it, every job waits in the queue for an existing
/// thread to free up. Counting `in_flight` against `total_threads`, rather than watching which
/// threads happen to be idle, makes the decision immediate and race-free: it does not depend on
/// how quickly an OS thread wakes from `recv`.
fn submit_job(pool: &Arc<ResolverPool>, job: ResolverJob) {
    let spawn_overflow = {
        let mut state = pool
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.in_flight += 1;
        let grow =
            state.in_flight > state.total_threads && state.total_threads < RESOLVER_POOL_CEILING;
        if grow {
            state.total_threads += 1;
        }
        grow
    };
    pool.queue.push(job);
    if spawn_overflow {
        let worker_pool = Arc::clone(pool);
        let spawned = std::thread::Builder::new().spawn(move || run_overflow_worker(&worker_pool));
        if spawned.is_err() {
            // The job is already queued for an existing thread to eventually pick up, so a
            // refused overflow thread isn't fatal — just undo the capacity bump counted above.
            pool.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .total_threads -= 1;
        }
    }
}

/// Run `lookup` on `pool`, and park the calling fiber on a fresh reactor token until a worker
/// wakes it with the result. The syscall runs on the pool's thread while the fiber that asked
/// for it is the only thing suspended — every other fiber, timer, and socket on the scheduler
/// keeps making progress. Split out from [`resolve_hostname`] so a test can drive an isolated
/// pool directly, rather than the lazily started global one.
fn resolve_hostname_on(
    pool: &Arc<ResolverPool>,
    address: &str,
    lookup: impl FnOnce(&str) -> io::Result<SocketAddr> + Send + 'static,
) -> io::Result<SocketAddr> {
    let (token, waker) = register_helper_waker();
    let (result_sender, result_receiver) = mpsc::channel();
    let job = ResolverJob {
        address: address.to_string(),
        lookup: Box::new(lookup),
        result_sender,
        waker,
        token,
    };
    submit_job(pool, job);
    park_on_readiness(token);
    result_receiver
        .recv()
        .expect("a resolver worker sends a result before waking the reactor")
}

/// `resolve`'s hostname path: resolve on the global [`resolver_pool`].
fn resolve_hostname(
    address: &str,
    lookup: impl FnOnce(&str) -> io::Result<SocketAddr> + Send + 'static,
) -> io::Result<SocketAddr> {
    let pool = resolver_pool()?;
    resolve_hostname_on(&pool, address, lookup)
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn a_panicking_lookup_wakes_the_fiber_with_an_error_instead_of_hanging_it() {
        // A helper thread that panics before storing a result and waking the reactor would
        // otherwise leave the parked fiber stuck forever; `resolve_hostname` must catch that
        // and still resolve to an `Err`.
        static RESOLVED_TO_ERROR: AtomicBool = AtomicBool::new(false);
        RESOLVED_TO_ERROR.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let lookup = |_: &str| -> io::Result<SocketAddr> {
                        panic!("a resolver hook that misbehaves");
                    };
                    let resolved = resolve_hostname("example.invalid:80", lookup);
                    RESOLVED_TO_ERROR.store(resolved.is_err(), Ordering::SeqCst);
                });
            });
        });

        assert!(
            RESOLVED_TO_ERROR.load(Ordering::SeqCst),
            "a panicking lookup must resolve to an error, not hang the fiber"
        );
    }

    #[test]
    fn several_concurrent_hostname_lookups_on_one_scheduler_all_resolve() {
        // More lookups than the pool could ever have threads for at once (whatever this
        // machine's CPU count clamps the base to), so some are guaranteed to queue behind
        // others: proves the pool serves every fiber's lookup, not just as many as it has
        // threads for, and hands each fiber back its own job's result rather than another's.
        const LOOKUPS: usize = RESOLVER_POOL_CEILING + 3;
        static RESOLVED_COUNT: AtomicUsize = AtomicUsize::new(0);
        RESOLVED_COUNT.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                for index in 0..LOOKUPS {
                    spawn(move || {
                        let lookup = move |_: &str| {
                            // A little work on the worker thread, so concurrent lookups
                            // actually overlap rather than finishing one at a time.
                            std::thread::sleep(Duration::from_millis(10));
                            Ok(SocketAddr::from(([127, 0, 0, 1], index as u16)))
                        };
                        let resolved = resolve_hostname("example.invalid:80", lookup)
                            .expect("every concurrent lookup must resolve");
                        assert_eq!(
                            resolved.port(),
                            index as u16,
                            "a fiber must get its own job's result back, not another's"
                        );
                        RESOLVED_COUNT.fetch_add(1, Ordering::SeqCst);
                    });
                }
            });
        });

        assert_eq!(RESOLVED_COUNT.load(Ordering::SeqCst), LOOKUPS);
    }

    /// A manual-reset gate: every lookup below blocks on [`ReleaseGate::wait`] until the test's
    /// controller thread calls [`ReleaseGate::release`], so the test can hold a known number of
    /// lookups in flight at once before letting any of them finish.
    struct ReleaseGate {
        released: Mutex<bool>,
        condvar: Condvar,
    }

    impl ReleaseGate {
        fn new() -> Self {
            ReleaseGate {
                released: Mutex::new(false),
                condvar: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut released = self.released.lock().unwrap_or_else(|p| p.into_inner());
            while !*released {
                released = self
                    .condvar
                    .wait(released)
                    .unwrap_or_else(|p| p.into_inner());
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap_or_else(|p| p.into_inner()) = true;
            self.condvar.notify_all();
        }
    }

    #[test]
    fn the_pool_grows_past_its_base_under_load_and_stops_at_the_ceiling() {
        // An isolated pool with its base forced small, so growth is observable regardless of
        // this machine's real CPU count; RESOLVER_POOL_CEILING is still the real ceiling
        // constant, shared with production.
        const BASE: usize = 4;
        const LOOKUPS: usize = RESOLVER_POOL_CEILING + 3;

        let pool = start_resolver_pool(BASE).expect("start an isolated pool for this test");
        let (started_sender, started_receiver) = mpsc::channel::<()>();
        let release_gate = Arc::new(ReleaseGate::new());

        static PEAK_THREADS: AtomicUsize = AtomicUsize::new(0);
        PEAK_THREADS.store(0, Ordering::SeqCst);

        // A real OS thread, concurrent with the scheduler's run() below (which is itself
        // synchronous and must not be the one waiting here): collect one "started" signal per
        // lookup that has actually begun running, sample the pool's thread count once enough
        // have started to prove the ceiling was reached, then release every blocked lookup so
        // run() can finish.
        let controller_pool = Arc::clone(&pool);
        let controller_gate = Arc::clone(&release_gate);
        let controller = std::thread::spawn(move || {
            for _ in 0..RESOLVER_POOL_CEILING {
                started_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("a lookup started");
            }
            // A short settle so a growth decision already in flight has finished before the
            // sample below reads it.
            std::thread::sleep(Duration::from_millis(50));
            PEAK_THREADS.store(
                controller_pool
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .total_threads,
                Ordering::SeqCst,
            );
            controller_gate.release();
            // Drain the rest of the "started" signals so every lookup has a chance to run.
            for _ in RESOLVER_POOL_CEILING..LOOKUPS {
                let _ = started_receiver.recv_timeout(Duration::from_secs(5));
            }
        });

        let scheduler_pool = Arc::clone(&pool);
        on_gc_thread(move || {
            run(move || {
                for _ in 0..LOOKUPS {
                    let pool = Arc::clone(&scheduler_pool);
                    let started_sender = started_sender.clone();
                    let release_gate = Arc::clone(&release_gate);
                    spawn(move || {
                        let lookup = move |_: &str| {
                            let _ = started_sender.send(());
                            release_gate.wait();
                            Ok(SocketAddr::from(([127, 0, 0, 1], 0)))
                        };
                        let _ = resolve_hostname_on(&pool, "example.invalid:80", lookup);
                    });
                }
            });
        });

        controller.join().expect("controller thread");
        assert_eq!(
            PEAK_THREADS.load(Ordering::SeqCst),
            RESOLVER_POOL_CEILING,
            "more concurrent lookups than the ceiling must grow the pool all the way to it"
        );

        // Every overflow thread exits once it finds the queue empty, but on a loaded machine
        // that can take a while (12 threads all contending for the same job-queue mutex right
        // after release); poll rather than trust one fixed sleep.
        let deadline = Instant::now() + Duration::from_secs(5);
        let final_thread_count = loop {
            let count = pool
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .total_threads;
            if count == BASE || Instant::now() >= deadline {
                break count;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            final_thread_count, BASE,
            "every overflow thread must have exited, leaving only the base"
        );
    }
}
