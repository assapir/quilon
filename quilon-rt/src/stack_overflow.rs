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
//! the default disposition and returns, letting the CPU re-execute the faulting
//! instruction — now fatal, with the original registers and program counter intact for
//! whatever reports the crash, exactly as an unhandled fault would look.
//!
//! Only the running fiber matters: a parked fiber's stack pointer is fixed until it
//! resumes, so only the one currently executing can newly fault on its own guard page.
//!
//! `sigaltstack` is a per-THREAD attribute, unlike `sigaction`'s process-wide
//! disposition, so [`install`] gives every thread that calls it its own — a compiled
//! program has exactly one, but a host running several programs' schedulers on separate
//! threads (the test suite, via `register_thread`) needs one per thread.

use crate::report::RUNTIME_EXIT_CODE;
use std::cell::Cell;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The currently running fiber's guard page, `[low, high)` — zero when no fiber is
/// running. Plain atomics, not the GC's mutex-guarded registry (`crate::gc`): the handler
/// runs on the thread it just interrupted and must never block on a lock that thread might
/// already hold.
static GUARD_LOW: AtomicUsize = AtomicUsize::new(0);
static GUARD_HIGH: AtomicUsize = AtomicUsize::new(0);

/// Record the running fiber's guard page before resuming it, returning whichever pair was
/// current before this call — pass it to [`restore_guard`] once the resume returns. A
/// resume can nest (`run_case_guarded` resumes a case's own fiber from inside a fiber
/// `run`'s ready-queue loop already resumed), so the outer fiber's guard has to come back,
/// not just get cleared, once the inner one is done — the outer fiber keeps running Quilon
/// code afterward, on its own stack, and a fault there is exactly the kind this module
/// exists to catch.
pub(crate) fn set_current_guard(low: usize, high: usize) -> (usize, usize) {
    let previous = (
        GUARD_LOW.load(Ordering::Relaxed),
        GUARD_HIGH.load(Ordering::Relaxed),
    );
    GUARD_LOW.store(low, Ordering::Relaxed);
    GUARD_HIGH.store(high, Ordering::Relaxed);
    previous
}

/// Put back the guard page [`set_current_guard`] returned, once its resume is done.
pub(crate) fn restore_guard(previous: (usize, usize)) {
    GUARD_LOW.store(previous.0, Ordering::Relaxed);
    GUARD_HIGH.store(previous.1, Ordering::Relaxed);
}

/// The plain-text report a stack overflow gets: no location, no color — a compile-time
/// constant rather than something built through the coded frame's `format!`/color-detection
/// machinery, neither of which the handler may safely call from inside a signal.
const MESSAGE: &[u8] = b"error[QN507]: stack overflow\n";

/// The signals a guard-page fault is delivered as. A `PROT_NONE` guard page always raises
/// `SIGSEGV` on Linux; macOS can raise either, depending on the faulting instruction, so
/// both are handled the same way.
const HANDLED_SIGNALS: [c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];

thread_local! {
    /// Whether THIS thread already has its own alternate signal stack. `sigaltstack`
    /// is per-thread, so every thread that ever runs a fiber scheduler needs one — see
    /// the module doc.
    static ALT_STACK_READY: Cell<bool> = const { Cell::new(false) };
}

/// Give this thread its own alternate signal stack, once — the fault the handler needs
/// to run for has no room left on the faulting stack. Idempotent per thread; a repeat
/// call on the same thread does nothing.
fn ensure_alt_stack() {
    ALT_STACK_READY.with(|ready| {
        if ready.get() {
            return;
        }
        ready.set(true);
        // Leaked for the thread's lifetime: the alternate stack must outlive every
        // signal it is ever used to handle.
        let stack_size = 64 * 1024;
        let stack: &'static mut [u8] = vec![0u8; stack_size].leak();
        let alternate_stack = libc::stack_t {
            ss_sp: stack.as_mut_ptr().cast(),
            ss_flags: 0,
            ss_size: stack_size,
        };
        // SAFETY: `alternate_stack` describes the just-leaked, live-for-the-thread buffer.
        unsafe { libc::sigaltstack(&alternate_stack, std::ptr::null_mut()) };
    });
}

/// Install the guard-page handler on this thread's alternate stack, and register it for
/// the process. Called from [`crate::gc::install_hooks`] on every thread that runs a
/// scheduler, before any fiber of its own runs; registering the process-wide signal
/// disposition again on a later call is harmless (the same handler, reinstalled).
///
/// Registers `SA_SIGINFO` (for the faulting address) `| SA_ONSTACK`.
pub(crate) fn install() {
    ensure_alt_stack();

    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle_signal as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    // SAFETY: `action` is otherwise a zeroed, valid `sigaction`; `sigemptyset` only writes
    // its `sa_mask` field.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    for signal in HANDLED_SIGNALS {
        // SAFETY: `action` is a fully initialized `sigaction`. The previous disposition is
        // discarded (never restored elsewhere): the handler itself resets `SIG_DFL` before
        // returning from anything it does not recognize, which is the same end state.
        unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
    }
}

/// The installed handler: report and exit when the fault landed in the running fiber's
/// guard page, otherwise fall back to the default disposition.
///
/// # Safety contract
/// Runs as a signal handler on the alternate stack [`install`] set up: every call inside
/// must be async-signal-safe. `MESSAGE` is a `'static` byte string (no allocation);
/// [`crate::io::write_to_fd`] loops on a raw `write(2)` (handling a short write, the one
/// realistic partial-progress case for a message this small) and never allocates; `_exit`
/// is a raw syscall. Resetting the disposition then returning — rather than calling
/// `raise` — lets the same faulting instruction retry, now fatal by the module doc's
/// contract.
extern "C" fn handle_signal(signal: c_int, info: *mut libc::siginfo_t, _context: *mut c_void) {
    // SAFETY: `info` is the siginfo the kernel handed the handler; reading the faulting
    // address neither allocates nor blocks.
    let fault_address = unsafe { (*info).si_addr() } as usize;
    let (low, high) = (
        GUARD_LOW.load(Ordering::Relaxed),
        GUARD_HIGH.load(Ordering::Relaxed),
    );

    if low != 0 && fault_address >= low && fault_address < high {
        crate::io::write_to_fd(2, MESSAGE);
        // SAFETY: `_exit(2)` neither allocates, locks, nor returns.
        unsafe { libc::_exit(RUNTIME_EXIT_CODE) };
    }

    // Not our guard page: restore the default action and return, so the CPU re-executes
    // the faulting instruction — now fatal, an unrelated crash exactly as it would have
    // been with no handler installed.
    let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
    default_action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: a fully initialized `sigaction` requesting the default disposition, for
    // the same signal the kernel just delivered.
    unsafe {
        libc::sigemptyset(&mut default_action.sa_mask);
        libc::sigaction(signal, &default_action, std::ptr::null_mut());
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
