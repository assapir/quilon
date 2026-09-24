// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The top-level signal trap (`!>`): `sigaction` is installed only for the signals a
//! program actually writes an arm for, only once a trap exists at all — a program with no
//! trap never calls [`__trap_install`], so nothing here ever runs for it.
//!
//! **Mechanism.** The signal handler itself runs only async-signal-safe code: it records
//! the sender (`si_pid`/`si_uid`) into a lock-free per-signal slot and writes one byte to
//! a self-pipe registered with the reactor — the same `mio`/reactor plumbing `net.rs`
//! registers a socket with (see `crate::scheduler::register_readiness`). A single
//! dispatcher fiber, spawned once (on the first arm installed), parks on the pipe's
//! readiness; woken, it drains the pipe and spawns a fresh fiber for every signal that
//! arrived and is not already running its arm. Per signal: a `running` flag and one
//! `pending` slot (a later arrival overwrites an earlier still-pending one, so at most one
//! is ever queued) — an arrival while the arm's own fiber is still running is delivered,
//! once, when that fiber returns; the returning fiber's own code notices the pending
//! arrival and spawns the next run itself, with no need to wake the dispatcher again (see
//! `spawn_run`, below).
//!
//! The dispatcher's own park is marked background (`crate::scheduler::mark_background_readiness`): a
//! trap stays installed for the life of the process, but that alone must never keep a
//! program with no other reason to keep running (a server, an open read, …) from exiting
//! once `^` returns — only an external signal or the program's own exit ends it otherwise.

use crate::scheduler::{
    mark_background_readiness, park_on_readiness, register_readiness, reregister_readiness, spawn,
};
use mio::Interest;
use mio::unix::pipe;
use std::io::Read;
use std::os::raw::{c_int, c_void};
use std::os::unix::io::AsRawFd;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

/// One entry per `process.Signal` variant, in EXACTLY the declaration order
/// `corelib/process.qn`'s `Signal` sum uses and codegen's own `SIGNAL_VARIANT_ORDER`
/// indexes by — the index `__trap_install` takes is never a raw OS signal number, which
/// differs across targets (Linux's `SIGUSR1`/`SIGUSR2` are 10/12; macOS's are 30/31).
const TRAP_SIGNALS: [c_int; 7] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGALRM,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

/// One signal's own state — touched by the (async-signal-unsafe-averse) handler and by
/// ordinary fiber-scheduler code, so every field is a plain atomic rather than behind a
/// lock the handler could deadlock on if it interrupted a holder.
struct Slot {
    /// Whether this signal's arm is currently running on its own fiber. Only ever
    /// written by ordinary scheduler-thread code (`dispatch_if_idle`/`spawn_run`), never
    /// by the signal handler.
    running: AtomicBool,
    /// A further arrival, to run once the current one (if any) finishes — at most one:
    /// a later arrival's sender overwrites an earlier still-pending one's.
    pending: AtomicBool,
    pending_pid: AtomicI64,
    pending_uid: AtomicI64,
    /// The arm's own generated function, `void (double pid, double uid)`, as a raw
    /// address — zero until installed (no arm written for this signal).
    handler: AtomicUsize,
}

const fn empty_slot() -> Slot {
    Slot {
        running: AtomicBool::new(false),
        pending: AtomicBool::new(false),
        pending_pid: AtomicI64::new(0),
        pending_uid: AtomicI64::new(0),
        handler: AtomicUsize::new(0),
    }
}

static SLOTS: [Slot; TRAP_SIGNALS.len()] = [
    empty_slot(),
    empty_slot(),
    empty_slot(),
    empty_slot(),
    empty_slot(),
    empty_slot(),
    empty_slot(),
];

/// The self-pipe's write end, as a raw descriptor the async-signal-safe handler writes to
/// directly (never through `mio`'s own `Write`, whose error path is not documented
/// async-signal-safe) — `-1` until [`ensure_dispatcher`] creates it.
static PIPE_WRITE_FD: AtomicI64 = AtomicI64::new(-1);

/// `__trap_install(signalIndex, armFn)`: install `armFn` for `TRAP_SIGNALS[signalIndex]`,
/// setting up the shared self-pipe and dispatcher fiber on the very first call. Codegen
/// calls this once per written arm, from `main`, before `^` runs (see
/// `CodeGenerator::generate_main_wrapper`) — a program with no trap never calls this.
///
/// # Safety contract (upheld by the compiler)
/// `arm_fn` is a live `extern "C" fn(f64, f64)` for the rest of the process's life.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __trap_install(signal_index: f64, arm_fn: *const c_void) {
    let index = signal_index as usize;
    assert!(
        index < TRAP_SIGNALS.len(),
        "__trap_install: signal index {index} out of range"
    );
    SLOTS[index]
        .handler
        .store(arm_fn as usize, Ordering::SeqCst);
    ensure_dispatcher();
    install_sigaction(TRAP_SIGNALS[index]);
}

/// Install `sigaction` for `signal` with [`handle_signal`], `SA_SIGINFO` only (no
/// `SA_ONSTACK` — unlike `stack_overflow`'s guard-page handler, this one only ever writes
/// a handful of atomics and a single byte, never touching the stack deeply enough to need
/// one).
fn install_sigaction(signal: c_int) {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle_signal as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO;
    // SAFETY: `action` is otherwise a zeroed, valid `sigaction`; `sigemptyset` only
    // writes its `sa_mask` field.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // SAFETY: `action` is a fully initialized `sigaction` for a real signal number.
    unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
}

/// The handler shared by every trapped signal — async-signal-safe only: record the
/// sender into `signal`'s slot and wake the self-pipe. Never runs Quilon code directly;
/// the dispatcher fiber (ordinary, non-signal-handler code) does that once woken.
///
/// # Safety contract
/// Runs as a signal handler: every call inside must be async-signal-safe. Reading
/// `si_pid`/`si_uid` off the kernel-supplied `siginfo_t` and storing into plain atomics
/// neither allocates nor blocks; the raw `write(2)` below is the same one-byte,
/// best-effort write [`crate::stack_overflow`]'s handler already relies on being safe.
extern "C" fn handle_signal(signal: c_int, info: *mut libc::siginfo_t, _context: *mut c_void) {
    let Some(index) = TRAP_SIGNALS.iter().position(|&known| known == signal) else {
        return;
    };
    // SAFETY: `info` is the siginfo the kernel handed the handler for this delivery.
    let (pid, uid) = unsafe { ((*info).si_pid(), (*info).si_uid()) };
    let slot = &SLOTS[index];
    slot.pending_pid.store(i64::from(pid), Ordering::SeqCst);
    slot.pending_uid.store(i64::from(uid), Ordering::SeqCst);
    slot.pending.store(true, Ordering::SeqCst);

    let fd = PIPE_WRITE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: `fd` is the self-pipe's write end, open for the rest of the process's
        // life once `ensure_dispatcher` creates it; writing one byte to a pipe the
        // dispatcher fiber keeps drained neither allocates, blocks, nor overflows it.
        unsafe {
            libc::write(fd as c_int, [1u8].as_ptr().cast(), 1);
        }
    }
}

/// Create the self-pipe and spawn the dispatcher fiber, once for the life of the process
/// — every further `__trap_install` call (a program's further arms) just adds another
/// signal onto the same pipe and dispatcher.
fn ensure_dispatcher() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let (sender, receiver) = pipe::new().expect("trap: failed to create the self-pipe");
        PIPE_WRITE_FD.store(i64::from(sender.as_raw_fd()), Ordering::SeqCst);
        // The write end is never read from `sender` itself again (the handler writes the
        // raw descriptor directly, async-signal-safety being the reason) and is never
        // closed — it lives for the rest of the process, exactly like the handler that
        // writes to it.
        std::mem::forget(sender);

        spawn(move || run_dispatcher(receiver));
    });
}

/// The trap dispatcher: parks on the self-pipe's readiness, drains it, then spawns a
/// fresh fiber for every signal that arrived and is not already running its arm — one
/// that arrived WHILE its own arm was already running is left for that fiber's own
/// finishing code to notice and re-spawn (see [`spawn_run`]), never here.
fn run_dispatcher(mut receiver: pipe::Receiver) -> ! {
    let token =
        register_readiness(&mut receiver, Interest::READABLE).expect("trap: register self-pipe");
    mark_background_readiness(token);

    let mut buffer = [0u8; 64];
    loop {
        reregister_readiness(&mut receiver, token, Interest::READABLE)
            .expect("trap: reregister self-pipe");
        park_on_readiness(token);
        // Drain every byte so the pipe never fills (a full pipe would block the signal
        // handler's write, which async-signal-safe code must never risk) — a wake needs
        // no more than "something arrived", not how many bytes said so.
        while matches!(receiver.read(&mut buffer), Ok(n) if n > 0) {}

        for index in 0..TRAP_SIGNALS.len() {
            dispatch_if_idle(index);
        }
    }
}

/// If signal `index` has a pending arrival and its arm is not already running, claim it
/// and spawn a fresh fiber to run it.
fn dispatch_if_idle(index: usize) {
    let slot = &SLOTS[index];
    if slot.running.load(Ordering::SeqCst) {
        return;
    }
    if !slot.pending.swap(false, Ordering::SeqCst) {
        return;
    }
    slot.running.store(true, Ordering::SeqCst);
    let pid = slot.pending_pid.load(Ordering::SeqCst) as f64;
    let uid = slot.pending_uid.load(Ordering::SeqCst) as f64;
    spawn_run(index, pid, uid);
}

/// Run signal `index`'s arm with sender `(pid, uid)` on a fresh fiber; once it returns,
/// check for a further arrival that came in while it ran and, if there is one, run it too
/// the same way — the "delivered once the body returns" rule, with no need to wake the
/// dispatcher again.
fn spawn_run(index: usize, pid: f64, uid: f64) {
    let handler = SLOTS[index].handler.load(Ordering::SeqCst);
    spawn(move || {
        if handler != 0 {
            // SAFETY: `__trap_install` only ever stores a live `extern "C" fn(f64,
            // f64)` here — codegen's own generated arm function.
            let arm_fn: extern "C" fn(f64, f64) = unsafe { std::mem::transmute(handler) };
            arm_fn(pid, uid);
        }
        SLOTS[index].running.store(false, Ordering::SeqCst);
        if SLOTS[index].pending.swap(false, Ordering::SeqCst) {
            let pid = SLOTS[index].pending_pid.load(Ordering::SeqCst) as f64;
            let uid = SLOTS[index].pending_uid.load(Ordering::SeqCst) as f64;
            SLOTS[index].running.store(true, Ordering::SeqCst);
            spawn_run(index, pid, uid);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// The pending-slot logic in isolation, with no real signal or fiber involved: an
    /// arrival sets `pending`+the sender, a second arrival before it is claimed overwrites
    /// the sender (never more than one pending), and claiming clears it.
    #[test]
    fn pending_slot_overwrites_and_claims() {
        let slot = empty_slot();
        assert!(!slot.pending.load(Ordering::SeqCst));

        slot.pending_pid.store(11, Ordering::SeqCst);
        slot.pending_uid.store(22, Ordering::SeqCst);
        slot.pending.store(true, Ordering::SeqCst);

        // A second arrival before the first is claimed overwrites the sender — at most
        // one pending, ever.
        slot.pending_pid.store(33, Ordering::SeqCst);
        slot.pending_uid.store(44, Ordering::SeqCst);
        slot.pending.store(true, Ordering::SeqCst);

        assert!(slot.pending.swap(false, Ordering::SeqCst));
        assert_eq!(slot.pending_pid.load(Ordering::SeqCst), 33);
        assert_eq!(slot.pending_uid.load(Ordering::SeqCst), 44);

        // Claimed: a further check finds nothing pending until another arrival sets it.
        assert!(!slot.pending.swap(false, Ordering::SeqCst));
    }

    /// `TRAP_SIGNALS` must have as many entries as `process.Signal` has variants (see the
    /// module doc): a mismatch here would silently misindex `__trap_install`'s calls.
    #[test]
    fn seven_trap_signals() {
        assert_eq!(TRAP_SIGNALS.len(), 7);
    }
}
