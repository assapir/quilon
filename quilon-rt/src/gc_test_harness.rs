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

use crate::gc;
use crate::test_support::GC_LOCK;
use std::os::raw::{c_int, c_void};
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
                job();
            }
        });
        sender
    })
}

/// Run `f` on the persistent GC worker, serialized against every other GC-touching
/// test via `GC_LOCK`, and block until it completes.
pub(crate) fn on_gc_thread<F: FnOnce() + Send + 'static>(f: F) {
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
