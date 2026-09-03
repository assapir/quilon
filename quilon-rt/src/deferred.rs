// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Deferred values — the runtime half of Quilon's colorless implicit futures.
//!
//! A value-returning `@` primitive does not park the calling fiber and hand back bytes;
//! it *launches* the IO on a background fiber and returns a **deferred value** immediately. The
//! value flows through the program as an ordinary one (a `Text`, here) and is *forced*
//! only when a strict primitive is about to read its concrete bytes. Forcing parks the
//! current fiber until the producing fiber has stored the result, then reads it — memoized,
//! so a second force is O(1).
//!
//! [`__read_launch`] backs `@read` (read one line from stdin): it allocates a [`Deferred`],
//! spawns a reader fiber that parks on stdin readiness and fills the cell, and returns the
//! deferred [`QlSlice`] representation. [`__force_text`] is the force: park-until-ready,
//! then return the stored bytes.
//!
//! Representation (hybrid, per the concurrency design): a `Text` is a `{ ptr, i64 }`
//! `QlSlice`. A *ready* `Text` carries its byte length (`>= 0`) in the second field; a
//! *deferred* `Text` carries [`DEFERRED_SENTINEL`] (`-1`) there and the deferred pointer in
//! the first — a real byte length is never negative, so the two are unambiguous. The code
//! generator forces exactly at the strict-use sites the deferred-taint pass marks, and only
//! for values that pass can be deferred; pure code never sees a sentinel and pays nothing.
//!
//! GC: the deferred cell is GC-allocated so its stored `data` pointer keeps the result bytes
//! alive; the reader fiber holds the cell on its (GC-scanned) stack from launch until it
//! returns, and the forcing fiber holds it across the park — so a collection at any point in
//! the deferred lifetime finds it. (A parked fiber's stack is scanned by the collector's
//! push-roots callback; `scheduler`/`net` tests prove that scanning directly.)
//!
//! Stdin is a single serial stream, so all reads are serialized through a **gate**: a reader
//! acquires the gate before it touches fd 0 and releases it when done, so at most one reader
//! ever owns the descriptor and the shared line buffer at a time. Two concurrent `@readStdin`
//! calls therefore read consecutive lines in launch order rather than racing the fd.

use crate::mem::{__alloc, QlSlice, alloc_text};
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
use crate::scheduler::{
    deregister_readiness, park_on_address, park_on_readiness, register_readiness,
    reregister_readiness, spawn, wake_address,
};
use mio::unix::SourceFd;
use mio::{Interest, Token};
use std::cell::{Cell, RefCell};
use std::io;
use std::os::raw::c_void;
use std::ptr;

/// The second (`i64`) field of a deferred `Text`'s `QlSlice`: a real byte length is never
/// negative, so `-1` unambiguously flags "the first field is a deferred pointer, not data".
/// The code generator's force check compares against this exact value.
pub const DEFERRED_SENTINEL: i64 = -1;

/// A Quilon `Result` whose payload is a `Text`, in the code generator's canonical layout
/// (`{ i8 tag, {ptr,i64} slot }`): a `Text` is itself a `{ptr,i64}`, so it fills the slot
/// directly with no boxing. `#[repr(C)]` puts the tag at offset 0 and the slot at offset 8 —
/// the same offsets LLVM emits for `{ i8, {ptr,i64} }` — so the two representations agree by
/// construction. The tags are the code generator's built-in Result discriminants.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QlResult {
    pub(crate) tag: i8,
    pub(crate) slot: QlSlice,
}

/// The `Ok` discriminant — the code generator's built-in Result tag for the success variant.
pub(crate) const RESULT_OK_TAG: i8 = 0;
/// The `NotOk` discriminant — the code generator's built-in Result tag for the failure variant.
pub(crate) const RESULT_NOTOK_TAG: i8 = 1;
/// The tag a DEFERRED `Result` carries in place of `Ok`/`NotOk`, with the deferred cell pointer
/// stashed in its slot's `data` field; the code generator's force check compares against it.
/// Distinct from every real discriminant, so a ready `Result` is never mistaken for a deferred
/// one.
pub const DEFERRED_RESULT_TAG: i8 = -1;

impl QlResult {
    /// A ready `Ok(text)` carrying `bytes` as its `Text` payload.
    pub(crate) fn ok(bytes: &[u8]) -> QlResult {
        QlResult {
            tag: RESULT_OK_TAG,
            slot: alloc_text(bytes),
        }
    }

    /// A ready `NotOk(message)` carrying `message` as its `Text` payload.
    pub(crate) fn not_ok(message: &str) -> QlResult {
        QlResult {
            tag: RESULT_NOTOK_TAG,
            slot: alloc_text(message.as_bytes()),
        }
    }
}

/// A deferred value's lifecycle. Carrying the resolved value INSIDE `Ready` makes
/// "ready but value absent" unrepresentable — a whole class of bug a bool/flag plus a
/// separate value field would allow. (Distinct from [`DEFERRED_SENTINEL`], which is the
/// C-ABI representation tag, not a lifecycle state.)
enum DeferredState<T> {
    /// The producer has not finished yet.
    Pending,
    /// The producer stored its result.
    Ready(T),
}

/// A deferred value's cell — the generic core every value-returning `@` primitive shares.
/// GC-allocated so any GC pointer inside a `Ready` value stays scannable; single-threaded and
/// cooperative, so its `state` needs no synchronization (the one producer fiber writes it
/// once, forcing fibers read after a wake). Opaque to the code generator, which only ever
/// carries the cell pointer around and hands it back to a `force` intrinsic.
pub(crate) struct Deferred<T> {
    state: DeferredState<T>,
}

/// Launch `producer` on a background fiber and return its deferred cell immediately — eager
/// launch: the producer runs whether or not the result is ever forced. The producer parks
/// however it needs (readiness, the stdin gate, ...); when it returns, its value is stored and
/// every forcing fiber woken. Generic over the produced type, so a new value-returning `@`
/// primitive (file/socket/HTTP) is just a different producer — no new park/deferred plumbing.
///
/// The cell is GC-allocated so a GC pointer inside `T` is scanned, and the producer fiber
/// holds the cell on its own (GC-scanned) stack until it returns, so the cell — and the value
/// it will hold — stay reachable across any collection while pending.
pub(crate) fn launch<T: 'static>(producer: impl FnOnce() -> T + 'static) -> *mut Deferred<T> {
    let size = std::mem::size_of::<Deferred<T>>();
    let cell = __alloc(size as i64) as *mut Deferred<T>;
    // The cell must be a fresh, GC-zeroed allocation, never a live one we would clobber.
    // `GC_malloc` zeroes, so a fresh cell reads as all-zero bytes; reading the not-yet-typed
    // memory as `u8` is well-defined (no invalid bit patterns), unlike reading it as `T`.
    // Debug-only; `__alloc` aborts rather than handing back null, so the read is safe.
    debug_assert!(
        unsafe { std::slice::from_raw_parts(cell as *const u8, size) }
            .iter()
            .all(|&byte| byte == 0),
        "initializing a Deferred cell that is not fresh"
    );
    // SAFETY: `__alloc` returned a fresh, aligned cell of exactly this size; initialize it
    // before anyone can observe it (`ptr::write`, since the memory is uninitialized as `T`).
    unsafe {
        ptr::write(
            cell,
            Deferred {
                state: DeferredState::Pending,
            },
        );
    }
    let address = cell as usize;
    spawn(move || {
        let value = producer();
        // A cell is resolved exactly once; a second resolve would clobber live data (and, once
        // M:N lands, signal a real race). Guard it in debug builds.
        debug_assert!(
            matches!(unsafe { &(*cell).state }, DeferredState::Pending),
            "resolving an already-resolved Deferred"
        );
        // SAFETY: `cell` is still the live cell this closure owns; single-threaded, so storing
        // the result cannot race a force. The assignment drops the prior `Pending` (a no-op).
        unsafe {
            (*cell).state = DeferredState::Ready(value);
        }
        wake_address(address);
    });
    cell
}

/// Force a deferred value: park the current fiber until `cell` is resolved, then return the
/// value (memoized — a second force is O(1)). `T: Copy` so the value is read out without
/// disturbing the cell that other forces still read.
///
/// # Safety
/// `cell` is a live deferred for the whole force (the taint pass keeps it reachable to here).
pub(crate) unsafe fn force<T: Copy>(cell: *mut Deferred<T>) -> T {
    let address = cell as usize;
    loop {
        // Re-read every iteration: a wake is an invitation to look, not a guarantee, and the
        // producer's store happens-before our resume (cooperative single thread).
        // SAFETY: `cell` is a live deferred (see the contract).
        match unsafe { &(*cell).state } {
            // `T: Copy`, so this reads the value out without disturbing the cell that other
            // forces still read (memoized).
            DeferredState::Ready(value) => return *value,
            DeferredState::Pending => park_on_address(address),
        }
    }
}

/// Launch a `Text`-producing IO on a background fiber and return its DEFERRED `Text`
/// representation (`{ deferred, -1 }`) immediately — the C-ABI wrapper over the generic
/// [`launch`] that EVERY value-returning `@` primitive shares (`@readStdin`, `@tcpRequest`), so
/// none re-copies the sentinel-tagging. The result threads through the program as an ordinary
/// `Text`; the code generator forces it (via [`__force_text`]) at its strict-use site.
pub(crate) fn launch_deferred_text(producer: impl FnOnce() -> QlSlice + 'static) -> QlSlice {
    let cell = launch(producer);
    QlSlice {
        data: cell as *const c_void,
        len: DEFERRED_SENTINEL,
    }
}

/// Launch a `Result`-producing IO on a background fiber and return its DEFERRED `Result`
/// representation immediately — the C-ABI wrapper over the generic [`launch`] for a
/// value-returning `@` primitive whose result is a `Result` (`@tcpRequest`), so its Ok/NotOk
/// wrapping and force plumbing are shared, not re-copied. The deferred representation is a
/// `Result` value tagged [`DEFERRED_RESULT_TAG`] with the deferred cell in its slot's `data`
/// field; the result threads through the program as an ordinary `Result` and the code generator
/// forces it (via [`__force_result`]) at the strict use that reads it.
pub(crate) fn launch_deferred_result(producer: impl FnOnce() -> QlResult + 'static) -> QlResult {
    let cell = launch(producer);
    QlResult {
        tag: DEFERRED_RESULT_TAG,
        slot: QlSlice {
            data: cell as *const c_void,
            len: 0,
        },
    }
}

/// Force a deferred `Result`, writing the resolved `{ tag, slot }` into `out`: the
/// per-representation C-ABI wrapper over the generic [`force`]. A `Result` is 24 bytes, which the
/// C ABI returns via a hidden pointer rather than in registers (unlike the 16-byte `Text`), so
/// the value is passed back through an out-pointer the code generator supplies — no aggregate
/// return crosses the FFI boundary. Only the code generator calls this, and only after its force
/// check saw [`DEFERRED_RESULT_TAG`], so `deferred_ptr` is always a live `Result` deferred.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QlResult`]; `deferred_ptr` is the slot `data` of a
/// deferred `Result` produced by [`launch_deferred_result`] and is still reachable (the taint pass
/// keeps it live to here).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_result(out: *mut QlResult, deferred_ptr: *const c_void) {
    // SAFETY: per the contract, this is a live `Result` deferred (a `Deferred<QlResult>`).
    let value = unsafe { force(deferred_ptr as *mut Deferred<QlResult>) };
    // SAFETY: `out` is writable storage for one `QlResult` (the code generator's alloca).
    unsafe { *out = value };
}

/// `@readStdin()`: launch a background read of one line from stdin and return the deferred
/// `Text` immediately (the calling fiber does not park here). A THIN wrapper over the shared
/// [`launch_deferred_text`], with a stdin-specific producer. `site` is the call's own location,
/// used to frame a fault report; it may be null if unknown.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a [`QlSite`] constant that outlives the program (the code
/// generator emits one read-only global per call site).
#[unsafe(no_mangle)]
pub extern "C" fn __read_launch(site: *const QlSite) -> QlSlice {
    // The site is a read-only constant in the module, so it outlives the launched read.
    launch_deferred_text(move || read_stdin_text(site))
}

/// Force a deferred `Text`: the per-representation C-ABI wrapper over the generic [`force`].
/// Only the code generator calls this, and only after its force check saw [`DEFERRED_SENTINEL`],
/// so `deferred_ptr` is always a live `Text` deferred.
///
/// # Safety contract (upheld by the compiler)
/// `deferred_ptr` is a pointer previously returned in the first field of a deferred
/// `__read_launch` result and is still reachable (the taint pass keeps it live to here).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_text(deferred_ptr: *const c_void) -> QlSlice {
    // SAFETY: per the contract, this is a live `Text` deferred (a `Deferred<QlSlice>`).
    unsafe { force(deferred_ptr as *mut Deferred<QlSlice>) }
}

/// The `@readStdin` producer: read one line from stdin as a `Text`, serialized on the stdin
/// gate so concurrent reads take consecutive lines rather than racing fd 0. Yields the empty
/// `Text` at end-of-input; a genuine IO error faults at the launch site (fail-loud).
fn read_stdin_text(site: *const QlSite) -> QlSlice {
    acquire_stdin();
    let read = read_stdin_line();
    release_stdin();
    match read {
        Ok(bytes) => alloc_text(&bytes),
        Err(error) => fail_read(site, &error),
    }
}

/// Report a fatal stdin read error at the `@readStdin` launch site, then terminate the
/// process (fail-loud). A genuine IO error on stdin is neither EOF nor `WouldBlock`.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
fn fail_read(site: *const QlSite, error: &io::Error) -> ! {
    fail_at(
        site,
        codes::READ_FAILED,
        &format!("@readStdin failed: {error}"),
        RUNTIME_EXIT_CODE,
    )
}

thread_local! {
    /// Bytes read past the newline of the previous `@readStdin`, kept so the next call
    /// continues the same stdin stream line-by-line rather than dropping them.
    static STDIN_LEFTOVER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// Whether a reader currently owns stdin (the gate). See [`acquire_stdin`].
    static STDIN_BUSY: Cell<bool> = const { Cell::new(false) };
}

const STDIN_FD: i32 = 0;

/// The stdin gate's wake address: a fixed sentinel (the address of a unique static), distinct
/// from any deferred cell (a heap pointer), so `park_on_address`/`wake_address` on it never
/// collide with a deferred.
fn stdin_gate() -> usize {
    static GATE: u8 = 0;
    &GATE as *const u8 as usize
}

/// Take ownership of stdin, waiting until any current reader releases it. Because the tier is
/// single-threaded and cooperative, the check-and-claim is atomic w.r.t. scheduling: a woken
/// waiter that finds the gate already re-taken simply parks again.
fn acquire_stdin() {
    while STDIN_BUSY.with(Cell::get) {
        park_on_address(stdin_gate());
    }
    STDIN_BUSY.with(|busy| busy.set(true));
}

/// Release stdin and wake every reader waiting for the gate; they re-contend, and the first to
/// run claims it.
fn release_stdin() {
    STDIN_BUSY.with(|busy| busy.set(false));
    wake_address(stdin_gate());
}

/// Read one line from stdin (fd 0), the source `@readStdin` reads. Thin wrapper over
/// [`read_line_from`]: it takes the persistent leftover buffer OUT of its thread-local while
/// reading (so no `RefCell` borrow is held across the fiber park) and stores what remains
/// back afterwards, so successive reads continue the same stream. The caller holds the stdin
/// gate, so there is exactly one reader in here at a time.
fn read_stdin_line() -> io::Result<Vec<u8>> {
    let mut buffer = STDIN_LEFTOVER.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    let result = read_line_from(STDIN_FD, &mut buffer);
    STDIN_LEFTOVER.with(|slot| *slot.borrow_mut() = buffer);
    result
}

/// Read one line from `fd` into `buffer`, parking the fiber on reactor readiness until a
/// newline arrives or the stream ends. Returns the line WITHOUT its trailing newline (a
/// trailing `\r` is dropped too). At end-of-input with nothing buffered, returns an empty
/// `Vec` — the documented end-of-input value (`@read` yields an empty `Text` there). Bytes
/// past the newline stay in `buffer` for the next call.
///
/// The reactor registration is LAZY: it reads first and only registers `fd` (and parks) on the
/// first `WouldBlock`. So a source that is ready right away — piped data already buffered, or a
/// non-pollable fd like a redirected file or `/dev/null` that returns data/EOF at once — never
/// touches `epoll`, which rejects such fds. Only a genuinely-not-ready pollable source (an
/// empty pipe/tty) is registered and parked on. Registering after a `WouldBlock` loses no
/// wakeup: adding an already-ready fd to the poll reports it immediately.
fn read_line_from(fd: i32, buffer: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    if let Some(line) = take_line(buffer) {
        return Ok(line);
    }
    set_nonblocking(fd);
    let mut source = SourceFd(&fd);
    let mut token: Option<Token> = None;
    let mut chunk = [0u8; 1024];
    let result = loop {
        // SAFETY: `read(2)` into a valid, owned buffer of `chunk.len()` bytes.
        let count = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut c_void, chunk.len()) };
        if count > 0 {
            buffer.extend_from_slice(&chunk[..count as usize]);
            if let Some(line) = take_line(buffer) {
                break Ok(line);
            }
        } else if count == 0 {
            // EOF: hand back whatever is buffered (an unterminated final line), or empty.
            break Ok(std::mem::take(buffer));
        } else {
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::WouldBlock => {
                    let active = match token {
                        Some(active) => {
                            match reregister_readiness(&mut source, active, Interest::READABLE) {
                                Ok(()) => active,
                                Err(error) => break Err(error),
                            }
                        }
                        None => match register_readiness(&mut source, Interest::READABLE) {
                            Ok(active) => {
                                token = Some(active);
                                active
                            }
                            Err(error) => break Err(error),
                        },
                    };
                    park_on_readiness(active);
                }
                io::ErrorKind::Interrupted => {}
                _ => break Err(error),
            }
        }
    };
    if token.is_some() {
        deregister_readiness(&mut source);
    }
    result
}

/// If `buffer` holds a complete line (up to and including a `\n`), remove and return it
/// without the newline (and without a preceding `\r`); otherwise `None`.
fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline = buffer.iter().position(|&byte| byte == b'\n')?;
    let mut line: Vec<u8> = buffer.drain(..=newline).collect();
    line.pop(); // drop the '\n'
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Put `fd` into non-blocking mode so a `read` on an empty pipe returns `WouldBlock` and
/// parks the fiber, rather than blocking the single OS thread. Failure is tolerable — a
/// still-blocking read simply blocks (functionally fine when nothing else is runnable).
fn set_nonblocking(fd: i32) {
    // SAFETY: `fcntl` on a descriptor; a bad fd just returns an error we ignore.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::os::raw::c_int;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }

    #[link(name = "gc", kind = "static")]
    unsafe extern "C" {
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }

    type Job = Box<dyn FnOnce() + Send>;

    // One persistent, Boehm-registered worker thread runs every GC-touching test body — the
    // same rationale as the `scheduler`/`net` test harnesses: a stable thread set keeps
    // stop-the-world signalling off exited threads.
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

    /// A `pipe(2)` pair, returned as `(read_end, write_end)`.
    fn make_pipe() -> (i32, i32) {
        let mut fds = [0i32; 2];
        // SAFETY: `pipe` fills a 2-element array with the two descriptors.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe() failed");
        (fds[0], fds[1])
    }

    /// Like [`__read_launch`] but reading one line from an arbitrary `fd` (a pipe), so a test
    /// can drive the producer with a controllable writer. Exercises the generic [`launch`] core
    /// with a pipe-reading producer and returns the deferred `{deferred, -1}` representation.
    fn launch_read_from_fd(fd: i32) -> QlSlice {
        launch_deferred_text(move || {
            let mut buffer = Vec::new();
            let bytes = read_line_from(fd, &mut buffer).expect("pipe read");
            alloc_text(&bytes)
        })
    }

    #[test]
    fn stdin_gate_serializes_readers() {
        // Two fibers contend for the stdin gate. The gate must keep at most one inside the
        // acquire/release section at a time (so concurrent `@readStdin` launches never race
        // fd 0), even though each yields (sleeps) while holding it.
        static CONCURRENT: AtomicUsize = AtomicUsize::new(0);
        static MAX_CONCURRENT: AtomicUsize = AtomicUsize::new(0);
        CONCURRENT.store(0, Ordering::SeqCst);
        MAX_CONCURRENT.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                for _ in 0..2 {
                    spawn(|| {
                        acquire_stdin();
                        let now = CONCURRENT.fetch_add(1, Ordering::SeqCst) + 1;
                        MAX_CONCURRENT.fetch_max(now, Ordering::SeqCst);
                        // Yield while holding the gate; a second reader must wait, not enter.
                        sleep(Duration::from_millis(10));
                        CONCURRENT.fetch_sub(1, Ordering::SeqCst);
                        release_stdin();
                    });
                }
            });
        });

        assert_eq!(
            MAX_CONCURRENT.load(Ordering::SeqCst),
            1,
            "the stdin gate let two readers hold stdin at once"
        );
    }

    #[test]
    fn take_line_splits_on_newline_and_keeps_remainder() {
        let mut buffer = b"first\r\nsecond".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"first".to_vec()));
        assert_eq!(buffer, b"second");
        // No newline yet: nothing to take.
        assert_eq!(take_line(&mut buffer), None);
        assert_eq!(buffer, b"second");
    }

    #[test]
    fn deferred_read_launches_and_forces_the_line() {
        // Proves the whole value-returning path: `@read` launches a background reader that
        // must PARK on stdin readiness (the writer is delayed), a separate fiber FORCES the
        // deferred value (parking on the deferred), and the read line flows through.
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        GOT.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let deferred_ptr = deferred.data;

                spawn(move || {
                    let forced = __force_text(deferred_ptr);
                    let bytes = unsafe {
                        std::slice::from_raw_parts(forced.data as *const u8, forced.len as usize)
                    };
                    *GOT.lock().unwrap() = bytes.to_vec();
                });

                // Delay the write so the reader is already parked on pipe readiness when it
                // arrives — the park path, not a lucky already-ready read, is what runs.
                spawn(move || {
                    sleep(Duration::from_millis(20));
                    let message = b"hello world\n";
                    unsafe {
                        libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
                    }
                });
            });
        });

        assert_eq!(&*GOT.lock().unwrap(), b"hello world");
    }

    #[test]
    fn force_is_memoized_after_ready() {
        // Forcing the same deferred value twice returns the same bytes, and the second force
        // never parks (the cell is already READY).
        static FIRST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static SECOND: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        FIRST.lock().unwrap().clear();
        SECOND.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            let message = b"line\n";
            // Write before the run so the value is ready without any park.
            unsafe {
                libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
            }
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let deferred_ptr = deferred.data;
                spawn(move || {
                    let a = __force_text(deferred_ptr);
                    let b = __force_text(deferred_ptr);
                    let read = |s: QlSlice| unsafe {
                        std::slice::from_raw_parts(s.data as *const u8, s.len as usize).to_vec()
                    };
                    *FIRST.lock().unwrap() = read(a);
                    *SECOND.lock().unwrap() = read(b);
                });
            });
        });

        assert_eq!(&*FIRST.lock().unwrap(), b"line");
        assert_eq!(&*SECOND.lock().unwrap(), b"line");
    }
}
