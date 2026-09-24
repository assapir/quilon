// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The top-level signal trap (`!>`): `sigaction` is installed only for the signals a
//! program actually traps, only once any exist.
//!
//! The handler is async-signal-safe only: it stores the sender in a per-signal slot and
//! wakes a self-pipe registered with the reactor. A dispatcher fiber drains the pipe and
//! spawns a fresh fiber per signal not already running its arm; an arrival mid-run is
//! delivered once, when that run returns. The dispatcher's own park is marked
//! background, so a trap alone never keeps an otherwise-idle program running.

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

/// `process.Signal`'s variants, in declaration order — an index into this, never a raw OS
/// signal number, since `SIGUSR1`/`SIGUSR2` differ by target (10/12 on Linux, 30/31 on macOS).
const TRAP_SIGNALS: [c_int; 7] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGALRM,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

/// Plain atomics, not a lock: the signal handler could interrupt a lock holder.
struct Slot {
    /// Set only by ordinary fiber code, never the signal handler.
    running: AtomicBool,
    /// A further arrival, to run once the current one finishes — a later one overwrites
    /// an earlier still-pending one, so at most one is ever queued.
    pending: AtomicBool,
    pending_pid: AtomicI64,
    pending_uid: AtomicI64,
    /// Zero until an arm is installed for this signal.
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

static SLOTS: [Slot; TRAP_SIGNALS.len()] = [const { empty_slot() }; TRAP_SIGNALS.len()];

/// A raw descriptor, not a `mio` handle: the handler writes to it directly, and `mio`'s
/// own `Write` has no documented async-signal-safety. `-1` until a trap installs.
static PIPE_WRITE_FD: AtomicI64 = AtomicI64::new(-1);

/// Installs `arm_fn` for `TRAP_SIGNALS[signal_index]`, setting up the shared self-pipe and
/// dispatcher fiber on the first call.
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

/// No `SA_ONSTACK`: the handler only touches a few atomics and a one-byte write, never
/// deep enough to need an alternate stack.
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

/// Shared by every trapped signal. Never runs Quilon code directly — only records the
/// sender and wakes the dispatcher.
///
/// # Safety contract
/// Every call inside must be async-signal-safe: reading `siginfo_t` and storing into
/// plain atomics neither allocates nor blocks, and the raw one-byte `write(2)` is
/// best-effort.
extern "C" fn handle_signal(signal: c_int, info: *mut libc::siginfo_t, _context: *mut c_void) {
    let Some(index) = TRAP_SIGNALS.iter().position(|&known| known == signal) else {
        return;
    };
    // SAFETY: `info` is the siginfo the kernel handed this delivery.
    let (pid, uid) = unsafe { ((*info).si_pid(), (*info).si_uid()) };
    let slot = &SLOTS[index];
    slot.pending_pid.store(i64::from(pid), Ordering::SeqCst);
    slot.pending_uid.store(i64::from(uid), Ordering::SeqCst);
    slot.pending.store(true, Ordering::SeqCst);

    let fd = PIPE_WRITE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: a one-byte write to a pipe the dispatcher keeps drained never blocks.
        unsafe {
            libc::write(fd as c_int, [1u8].as_ptr().cast(), 1);
        }
    }
}

/// Creates the self-pipe and spawns the dispatcher fiber, once.
fn ensure_dispatcher() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let (sender, receiver) = pipe::new().expect("trap: failed to create the self-pipe");
        PIPE_WRITE_FD.store(i64::from(sender.as_raw_fd()), Ordering::SeqCst);
        // Leaked: the signal handler writes the raw fd directly, for the rest of the
        // process's life.
        std::mem::forget(sender);

        spawn(move || run_dispatcher(receiver));
    });
}

/// Parks on the self-pipe's readiness, drains it, then dispatches every signal that
/// arrived and isn't already running.
fn run_dispatcher(mut receiver: pipe::Receiver) -> ! {
    let token =
        register_readiness(&mut receiver, Interest::READABLE).expect("trap: register self-pipe");
    mark_background_readiness(token);

    let mut buffer = [0u8; 64];
    loop {
        reregister_readiness(&mut receiver, token, Interest::READABLE)
            .expect("trap: reregister self-pipe");
        park_on_readiness(token);
        // Drain fully so the pipe never fills, which would block the handler's write.
        while matches!(receiver.read(&mut buffer), Ok(n) if n > 0) {}

        for index in 0..TRAP_SIGNALS.len() {
            dispatch_if_idle(index);
        }
    }
}

/// Claims a pending, not-already-running arrival and spawns its arm.
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

/// Runs the arm on a fresh fiber; on return, re-dispatches a pending arrival directly,
/// with no need to wake the dispatcher again.
fn spawn_run(index: usize, pid: f64, uid: f64) {
    let handler = SLOTS[index].handler.load(Ordering::SeqCst);
    spawn(move || {
        if handler != 0 {
            // SAFETY: only `__trap_install` ever stores here, a live arm function.
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

    #[test]
    fn pending_slot_overwrites_and_claims() {
        let slot = empty_slot();
        assert!(!slot.pending.load(Ordering::SeqCst));

        slot.pending_pid.store(11, Ordering::SeqCst);
        slot.pending_uid.store(22, Ordering::SeqCst);
        slot.pending.store(true, Ordering::SeqCst);

        // Overwrites the still-pending arrival above — at most one pending, ever.
        slot.pending_pid.store(33, Ordering::SeqCst);
        slot.pending_uid.store(44, Ordering::SeqCst);
        slot.pending.store(true, Ordering::SeqCst);

        assert!(slot.pending.swap(false, Ordering::SeqCst));
        assert_eq!(slot.pending_pid.load(Ordering::SeqCst), 33);
        assert_eq!(slot.pending_uid.load(Ordering::SeqCst), 44);
        assert!(!slot.pending.swap(false, Ordering::SeqCst));
    }

    /// Must match `process.Signal`'s variant count, or `__trap_install` misindexes.
    #[test]
    fn seven_trap_signals() {
        assert_eq!(TRAP_SIGNALS.len(), 7);
    }
}
