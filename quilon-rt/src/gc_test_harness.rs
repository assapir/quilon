// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Shared unit-test harness for GC-touching tests, used by `scheduler`, `net` (and its
//! `client`/`server` submodules), `deferred`, and `blocking`.
//!
//! A single, persistent, Boehm-registered worker thread runs every GC-touching test
//! body. Boehm's stop-the-world uses signals to suspend registered threads; if
//! collections ran on the ephemeral per-test threads the harness spawns, Boehm would
//! try to signal threads that have since exited and abort ("Signals delivery fails
//! constantly"). Funneling all fiber work onto one long-lived registered thread keeps
//! Boehm's thread set stable (main + worker).
//!
//! Consequence of sharing that one worker thread across four modules' tests: they also
//! share its thread-locals — `scheduler::SCHEDULER`/`REACTOR`,
//! `net::server::SERVERS`/`CONNECTIONS`/`NEXT_HANDLE`/`HANDLER_FIBER_SERVER`, and
//! `deferred::STDIN_BUSY`/`STDIN_LEFTOVER`. Every test today cleans up after itself, so
//! this passes, but it is a real coupling: a test that leaves `SERVERS` populated or
//! `STDIN_BUSY == true` can now poison a later test in a DIFFERENT module, where it
//! used to only ever run on its own module's dedicated worker. A new GC-touching test
//! must leave these thread-locals as it found them.

use crate::gc;
use crate::test_support::GC_LOCK;
use std::any::Any;
use std::os::raw::{c_int, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::OnceLock;
use std::sync::mpsc;

#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
    fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
}

#[repr(C)]
struct GcStackBase {
    mem_base: *mut c_void,
}

type Job = Box<dyn FnOnce() + Send>;

fn gc_worker() -> &'static mpsc::Sender<Job> {
    static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            // Hooks first (they call GC_allow_register_threads), then register
            // this worker once, for the life of the process.
            gc::install_hooks();
            let mut stack_base = GcStackBase {
                mem_base: ptr::null_mut(),
            };
            unsafe {
                GC_get_stack_base(&mut stack_base);
                GC_register_my_thread(&stack_base);
            }
            for job in receiver {
                // `job` (built in `on_gc_thread`) already catches its own test body's
                // panic and reports it back over its own done-channel before returning
                // normally, so this outer catch is a second line of defense — nothing in
                // `job`'s own bookkeeping is expected to panic. But this worker now runs
                // every GC-touching test in the crate, not just one module's, so if it
                // ever did, one test's panic must still not take the worker thread down
                // (and every remaining GC test with it) — it stays one failing test.
                let _ = panic::catch_unwind(AssertUnwindSafe(job));
            }
        });
        sender
    })
}

/// Run `f` on the persistent GC worker, serialized against every other GC-touching
/// test via `GC_LOCK`, and block until it completes. If `f` panics, that panic is
/// caught on the worker thread (so the worker survives for the next queued test) and
/// re-raised here on the calling thread via `resume_unwind`, so the test that called
/// `on_gc_thread` still fails, and fails with its own panic message — not a follow-on
/// `SendError` from some unrelated later test sharing the same worker.
pub(crate) fn on_gc_thread<F: FnOnce() + Send + 'static>(f: F) {
    let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (done_sender, done_receiver) = mpsc::channel::<Option<Box<dyn Any + Send>>>();
    gc_worker()
        .send(Box::new(move || {
            let outcome = panic::catch_unwind(AssertUnwindSafe(f));
            let _ = done_sender.send(outcome.err());
        }))
        .unwrap();
    if let Some(payload) = done_receiver.recv().unwrap() {
        panic::resume_unwind(payload);
    }
}
