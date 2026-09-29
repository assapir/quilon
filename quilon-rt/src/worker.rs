// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The current worker: the state that used to sit in seven separate thread-locals across
//! `scheduler`, `launch_scope`, `net::server`, and `abort_trap`, now owned by one [`Worker`]
//! per OS thread and reached only through [`with_worker`]/[`try_with_worker`] — never a bare
//! thread-local read. [`WORKER`] is a plain `thread_local!`; `scheduler::run` spawns one
//! `Worker` per OS thread (see `docs/concurrency/runtime.md`), and every caller reaches its
//! own through this module's accessors, never another thread's.
//!
//! Each field keeps its own interior mutability (a `Cell`/`RefCell` per field, not one
//! `RefCell` around the whole struct) — exactly mirroring the independent thread-locals it
//! replaces. [`with_worker`] only ever takes a shared `&Worker` (never `borrow_mut`s the
//! outer `Option`), so nesting two calls to it — already how the replaced thread-locals were
//! used relative to each other — stays panic-free: one field's own short-scoped borrow never
//! blocks access to a sibling field's.
//!
use crate::launch_scope::LaunchScope;
use crate::net::server::ConnectionState;
use crate::placement::Registry;
use crate::reactor::Reactor;
use crate::scheduler::{FiberYielder, Scheduler};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) struct Worker {
    /// This worker's own index into `registry` — `0` is always the leader.
    pub(crate) index: usize,
    /// Every worker of this `scheduler::run` call, for cross-worker placement.
    pub(crate) registry: Arc<Registry>,
    /// The run queue, timers, and readiness/address waiters. See `scheduler::Scheduler`.
    pub(crate) scheduler: RefCell<Scheduler>,
    /// This worker's `mio` poll wrapper.
    pub(crate) reactor: RefCell<Reactor>,
    /// The running fiber's `Yielder`.
    pub(crate) current_yielder: Cell<*const FiberYielder>,
    /// How many `aborts()` traps are in progress on this worker right now.
    pub(crate) abort_trap_depth: Cell<u32>,
    /// The ids of the fibers currently resuming on this worker, innermost last.
    pub(crate) running_fibers: RefCell<Vec<usize>>,
    /// Every fiber's own open launch-scope stack, keyed by fiber id. See `launch_scope`.
    pub(crate) launch_scopes: RefCell<HashMap<Option<usize>, Vec<LaunchScope>>>,
    /// The withheld report from this worker's most recent aborted `aborts()` trap.
    pub(crate) last_abort_report: RefCell<String>,
    /// This worker's own open connections, by handle id — only ever touched by the worker
    /// that accepted them (see `net::server`'s ownership-rule comment); a `Server` handle,
    /// killable from any worker, lives in `registry.servers()` instead.
    pub(crate) connections: RefCell<HashMap<u64, Rc<ConnectionState>>>,
    /// Which server's `in_flight` count each currently-running handler fiber counts against.
    pub(crate) handler_fiber_server: RefCell<HashMap<usize, usize>>,
}

impl Worker {
    fn new(index: usize, registry: Arc<Registry>, reactor: Reactor) -> Self {
        Worker {
            index,
            registry,
            scheduler: RefCell::new(Scheduler::new()),
            reactor: RefCell::new(reactor),
            current_yielder: Cell::new(std::ptr::null()),
            abort_trap_depth: Cell::new(0),
            running_fibers: RefCell::new(Vec::new()),
            launch_scopes: RefCell::new(HashMap::new()),
            last_abort_report: RefCell::new(String::new()),
            connections: RefCell::new(HashMap::new()),
            handler_fiber_server: RefCell::new(HashMap::new()),
        }
    }

    /// Publish this worker's current run-queue length to the registry, for another
    /// worker's shortest-queue placement decision to read. Called by `scheduler` every
    /// time its ready queue's length changes.
    pub(crate) fn sync_ready_len(&self) {
        let len = self.scheduler.borrow().ready_len();
        self.registry.set_ready_len(self.index, len);
    }
}

thread_local! {
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

/// Install this thread's worker at `index` in `registry`, built around `reactor`. Panics if
/// one is already installed — `scheduler::run`'s own "not re-entrant" rule, now enforced
/// here since installing the worker is what a worker thread's own startup calls first.
pub(crate) fn install(index: usize, registry: Arc<Registry>, reactor: Reactor) {
    WORKER.with(|worker| {
        let mut slot = worker.borrow_mut();
        assert!(slot.is_none(), "run() is not re-entrant");
        *slot = Some(Worker::new(index, registry, reactor));
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
