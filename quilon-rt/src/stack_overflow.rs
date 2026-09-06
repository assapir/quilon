// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Reporting a stack overflow instead of dying to a bare `SIGSEGV`.
//!
//! A fiber's stack is bounded by one guard page (see
//! [`crate::scheduler`]'s `spawn_with_stack`); recursion deep enough to reach it faults
//! there. [`install`] puts a signal handler on its own alternate stack — the fault happens
//! with no room left on the fiber's own stack — that checks the faulting address against
//! the currently RUNNING fiber's guard page and reports [`report::codes::STACK_OVERFLOW`]
//! through the runtime's usual [`report::RUNTIME_EXIT_CODE`] when it lands there. A fault
//! outside that page is not one the runtime knows how to explain, so the handler restores
//! the default disposition and re-raises, leaving it a genuine crash.
//!
//! Only the running fiber matters: a parked fiber's stack pointer is fixed until it
//! resumes, so only the one currently executing can newly fault on its own guard page.

use crate::report::RUNTIME_EXIT_CODE;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The currently running fiber's guard page, `[low, high)` — zero when no fiber is
/// running. Plain atomics, not the GC's mutex-guarded registry (`crate::gc`): the handler
/// runs on the thread it just interrupted and must never block on a lock that thread might
/// already hold.
static GUARD_LOW: AtomicUsize = AtomicUsize::new(0);
static GUARD_HIGH: AtomicUsize = AtomicUsize::new(0);

/// Record the running fiber's guard page before resuming it. Called from the scheduler
/// around every `coroutine.resume`.
pub(crate) fn set_current_guard(low: usize, high: usize) {
    GUARD_LOW.store(low, Ordering::Relaxed);
    GUARD_HIGH.store(high, Ordering::Relaxed);
}

/// Forget the guard page after a fiber yields or finishes, so a fault while no fiber is
/// running (the scheduler's own code, between two resumes) is never misattributed to the
/// last one.
pub(crate) fn clear_current_guard() {
    GUARD_LOW.store(0, Ordering::Relaxed);
    GUARD_HIGH.store(0, Ordering::Relaxed);
}

/// The plain-text report a stack overflow gets: no location, no color — a compile-time
/// constant rather than something built through the coded frame's `format!`/color-detection
/// machinery, neither of which the handler may safely call from inside a signal.
const MESSAGE: &[u8] = b"error[QN507]: stack overflow\n";

/// The signals a guard-page fault is delivered as. A `PROT_NONE` guard page always raises
/// `SIGSEGV` on Linux; macOS can raise either, depending on the faulting instruction, so
/// both are handled the same way.
const HANDLED_SIGNALS: [c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];

/// Install the guard-page handler once. Idempotent; called from [`crate::gc::install_hooks`]
/// before any fiber runs.
///
/// Puts the handler on an alternate signal stack (`sigaltstack`) — the faulting fiber's own
/// stack has no room left — and registers it with `SA_SIGINFO` (for the faulting address)
/// `| SA_ONSTACK`.
pub(crate) fn install() {
    // Leaked for the process's lifetime: the alternate stack must outlive every signal it
    // is ever used to handle.
    let stack_size = 64 * 1024;
    let stack: &'static mut [u8] = vec![0u8; stack_size].leak();
    let alternate_stack = libc::stack_t {
        ss_sp: stack.as_mut_ptr().cast(),
        ss_flags: 0,
        ss_size: stack_size,
    };
    // SAFETY: `alternate_stack` describes the just-leaked, live-for-the-process buffer.
    unsafe { libc::sigaltstack(&alternate_stack, std::ptr::null_mut()) };

    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle_signal as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    // SAFETY: `action` is otherwise a zeroed, valid `sigaction`; `sigemptyset` only writes
    // its `sa_mask` field.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    for signal in HANDLED_SIGNALS {
        // SAFETY: `action` is a fully initialized `sigaction`. The previous disposition is
        // discarded (never restored elsewhere): the handler itself resets `SIG_DFL` before
        // re-raising anything it does not recognize, which is the same end state.
        unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
    }
}

/// The installed handler: report and exit when the fault landed in the running fiber's
/// guard page, otherwise fall back to the default disposition.
///
/// # Safety contract
/// Runs as a signal handler on the alternate stack [`install`] set up: every call inside
/// must be async-signal-safe. `MESSAGE` is a `'static` byte string (no allocation),
/// `write`/`_exit` are raw syscalls, and resetting the disposition then re-raising is the
/// standard way to make an unhandled signal terminate the process as if this handler had
/// never run.
extern "C" fn handle_signal(signal: c_int, info: *mut libc::siginfo_t, _context: *mut c_void) {
    // SAFETY: `info` is the siginfo the kernel handed the handler; reading the faulting
    // address neither allocates nor blocks.
    let fault_address = unsafe { (*info).si_addr() } as usize;
    let (low, high) = (
        GUARD_LOW.load(Ordering::Relaxed),
        GUARD_HIGH.load(Ordering::Relaxed),
    );

    if low != 0 && fault_address >= low && fault_address < high {
        // SAFETY: a raw `write(2)` on a fixed byte string, then `_exit(2)` — neither
        // allocates, locks, nor returns.
        unsafe {
            libc::write(2, MESSAGE.as_ptr().cast(), MESSAGE.len());
            libc::_exit(RUNTIME_EXIT_CODE);
        }
    }

    // Not our guard page: restore the default action and re-raise, so an unrelated fault
    // still crashes the process the way it would have with no handler installed.
    let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
    default_action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: a fully initialized `sigaction` requesting the default disposition, installed
    // for the same signal the kernel just delivered, then re-raised on this thread.
    unsafe {
        libc::sigemptyset(&mut default_action.sa_mask);
        libc::sigaction(signal, &default_action, std::ptr::null_mut());
        libc::raise(signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_matches_the_registered_code() {
        assert_eq!(
            MESSAGE,
            format!(
                "error[QN{:03}]: stack overflow\n",
                crate::report::codes::STACK_OVERFLOW
            )
            .as_bytes()
        );
    }
}
