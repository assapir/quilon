// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The `aborts()` matcher's runtime half: run a zero-parameter lambda on a guarded fiber
//! (`crate::scheduler::run_abort_trap_guarded`) and report whether it ended in a fail-loud
//! exit instead of returning. Reuses the same nested-fiber mechanism `core.test`'s guarded
//! case body does (`crate::test_registry::__test_case_run_guarded`): while the trap is
//! active, `crate::report::fail_at` and `crate::process::__exit` suspend the fiber with the
//! exit code and the report withheld from stderr, rather than terminating the process.

use std::cell::RefCell;
use std::os::raw::c_void;

use crate::mem::{QlSlice, alloc_text};

thread_local! {
    /// The withheld report from the most recent `__abort_trap_run` that aborted — empty if
    /// it returned instead. Read once, right after that call, by `__abort_trap_report`; a
    /// later `__abort_trap_run` overwrites it, which is safe because codegen always reads it
    /// (if at all) before evaluating another `aborts()` matcher.
    static LAST_REPORT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Run the zero-parameter lambda `function(environment)` — the split-apart `{ ptr fn, ptr
/// env }` of the trampoline the code generator builds for `aborts()` (see
/// `CodeGenerator::generate_aborts_held`), not the lambda's own function pointer, whose
/// signature varies with its return type. Yields `1` if it ended in a fail-loud exit, `0` if
/// it returned; either way, the withheld report (if any) is left for [`__abort_trap_report`].
///
/// # Safety contract (upheld by the compiler)
/// `function`/`environment` are the function and environment pointers of a live `(ptr) ->
/// i8` closure value, exactly as the code generator's `aborts()` trampoline is shaped.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __abort_trap_run(function: *const c_void, environment: *mut c_void) -> u8 {
    // SAFETY: `function` is the function pointer of a live `(ptr) -> i8` trampoline (the
    // caller's contract, above), which is exactly this signature.
    let function: extern "C" fn(*mut c_void) -> u8 = unsafe { std::mem::transmute(function) };
    match crate::scheduler::run_abort_trap_guarded(function, environment) {
        Some(report) => {
            LAST_REPORT.with(|last| *last.borrow_mut() = report);
            1
        }
        None => {
            LAST_REPORT.with(|last| last.borrow_mut().clear());
            0
        }
    }
}

/// The withheld report from the `__abort_trap_run` that just aborted, as a `Text` — empty
/// when it returned instead. Backs a failing `not(aborts())`'s mismatch message.
#[unsafe(no_mangle)]
pub extern "C" fn __abort_trap_report() -> QlSlice {
    LAST_REPORT.with(|last| alloc_text(last.borrow().as_bytes()))
}

// See `scheduler`'s own test module for the trap's unit tests: they need a fiber (and, for
// `__abort_trap_report`, a GC-registered thread) to run on, which is the harness already
// built there for `run`/`spawn`.
