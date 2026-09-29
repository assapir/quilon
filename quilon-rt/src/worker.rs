// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The current worker: the state that used to sit in seven separate thread-locals across
//! `scheduler`, `launch_scope`, `net::server`, and `abort_trap`, now owned by one [`Worker`]
//! per OS thread and reached only through [`with_worker`]/[`try_with_worker`] — never a bare
//! thread-local read. This is a straight move, not a redesign: [`WORKER`] is still a plain
//! `thread_local!`, so today's runtime is still exactly one worker on exactly one thread. The
//! payoff comes later, when the M:N scheduler spawns one `Worker` per CPU: every caller
//! already goes through the accessor here, so giving each OS thread its own worker needs no
//! further change at those call sites.
//!
//! Each field keeps its own interior mutability (a `Cell`/`RefCell` per field, not one
//! `RefCell` around the whole struct) — exactly mirroring the independent thread-locals it
//! replaces. [`with_worker`] only ever takes a shared `&Worker` (never `borrow_mut`s the
//! outer `Option`), so nesting two calls to it — already how the replaced thread-locals were
//! used relative to each other — stays panic-free: one field's own short-scoped borrow never
//! blocks access to a sibling field's.
//!
//! `current_yielder` is stored as an untyped `*const c_void` rather than
//! `*const scheduler::FiberYielder`: `FiberYielder`'s definition depends on `scheduler`'s own
//! private `Park` enum, and this module has no reason to see either — `scheduler` casts on
//! its own read and write, the same way it always cast this raw pointer.

use crate::launch_scope::LaunchScope;
use crate::net::server::{ConnectionState, ServerState};
use crate::reactor::Reactor;
use crate::scheduler::Scheduler;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::rc::Rc;

pub(crate) struct Worker {
    /// The run queue, timers, and readiness/address waiters. See `scheduler::Scheduler`.
    pub(crate) scheduler: RefCell<Scheduler>,
    /// This worker's `mio` poll wrapper.
    pub(crate) reactor: RefCell<Reactor>,
    /// The running fiber's `Yielder` — see the module doc for why this is untyped here.
    pub(crate) current_yielder: Cell<*const c_void>,
    /// How many `aborts()` traps are in progress on this worker right now.
    pub(crate) abort_trap_depth: Cell<u32>,
    /// The ids of the fibers currently resuming on this worker, innermost last.
    pub(crate) running_fibers: RefCell<Vec<usize>>,
    /// Every fiber's own open launch-scope stack, keyed by fiber id (`None` for a call made
    /// outside any fiber). See `launch_scope`.
    pub(crate) launch_scopes: RefCell<HashMap<Option<usize>, Vec<LaunchScope>>>,
    /// The withheld report from the most recent `aborts()` trap that aborted on this worker,
    /// empty if it returned instead. See `abort_trap`.
    pub(crate) last_abort_report: RefCell<String>,
    /// The next `Connection`/`Server` handle id this worker hands out. See `net::server`.
    pub(crate) next_handle: Cell<u64>,
    /// `net.@tcpServe`'s open connections, by handle id.
    pub(crate) connections: RefCell<HashMap<u64, Rc<ConnectionState>>>,
    /// `net.@tcpServe`'s live servers, by handle id.
    pub(crate) servers: RefCell<HashMap<u64, Rc<ServerState>>>,
    /// Which server's `in_flight` count a currently-running handler fiber counts against,
    /// keyed by fiber id.
    pub(crate) handler_fiber_server: RefCell<HashMap<usize, usize>>,
}

impl Worker {
    fn new(reactor: Reactor) -> Self {
        Worker {
            scheduler: RefCell::new(Scheduler::new()),
            reactor: RefCell::new(reactor),
            current_yielder: Cell::new(std::ptr::null()),
            abort_trap_depth: Cell::new(0),
            running_fibers: RefCell::new(Vec::new()),
            launch_scopes: RefCell::new(HashMap::new()),
            last_abort_report: RefCell::new(String::new()),
            next_handle: Cell::new(1),
            connections: RefCell::new(HashMap::new()),
            servers: RefCell::new(HashMap::new()),
            handler_fiber_server: RefCell::new(HashMap::new()),
        }
    }
}

thread_local! {
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

/// Install this thread's worker, built around `reactor`. Panics if one is already installed —
/// `scheduler::run`'s own "not re-entrant" rule, now enforced here since installing the
/// worker is what `run` calls first.
pub(crate) fn install(reactor: Reactor) {
    WORKER.with(|worker| {
        let mut slot = worker.borrow_mut();
        assert!(slot.is_none(), "run() is not re-entrant");
        *slot = Some(Worker::new(reactor));
    });
}

/// Tear down this thread's worker at the end of its `run()`.
pub(crate) fn teardown() {
    WORKER.with(|worker| *worker.borrow_mut() = None);
}

/// Run `f` against this thread's worker, asserting one is installed. A shared borrow of the
/// outer `Option` only (see the module doc), so calling this again from inside `f` is safe.
pub(crate) fn with_worker<R>(f: impl FnOnce(&Worker) -> R) -> R {
    WORKER.with(|worker| f(worker.borrow().as_ref().expect("no active worker")))
}

/// Like [`with_worker`], but `None` rather than a panic when no worker is installed — for a
/// caller that must tolerate running outside a `run()` (e.g. a source dropped after the run
/// that registered it has already torn its worker down).
pub(crate) fn try_with_worker<R>(f: impl FnOnce(&Worker) -> R) -> Option<R> {
    WORKER.with(|worker| worker.borrow().as_ref().map(f))
}
