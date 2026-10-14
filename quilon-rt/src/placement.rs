// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Cross-worker placement: the shortest-run-queue pick for a new launch or an accepted
//! connection, and the inbox each worker's own thread drains to receive one.
//!
//! One [`Registry`] is built per `scheduler::run` call and shared (`Arc`) by every worker of
//! that run. The leader (worker 0) publishes itself and runs alone; helper threads start
//! lazily, on the first placement decision (`scheduler::spawn_placed`'s own
//! `ensure_helpers_started`), so a program that never launches anything never pays for them.
//! That first call spawns every helper, waits (a `Barrier` sized to the worker count, the
//! caller its own participant) until each has published its [`WorkerHandle`], then proceeds
//! — so [`Registry::shortest_queue`] never reads an unpublished slot. [`Registry::place`]
//! queues a job in the chosen worker's inbox and wakes its reactor through the same
//! [`ReactorWaker`] mechanism a blocking-pool job's completion already uses.

use crate::net::server::ServerState;
use crate::reactor::{Reactor, ReactorWaker};
use mio::Token;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

/// A unit of work handed to a worker other than the one that created it. Already carries
/// everything it needs — the receiving worker's own thread just calls it.
pub(crate) type Job = Box<dyn FnOnce() + Send>;

struct WorkerHandle {
    /// Kept in step by `scheduler`'s own ready-queue push/pop points.
    ready_len: AtomicUsize,
    /// Jobs placed but not yet drained — counted alongside `ready_len` in
    /// [`shortest_queue`](Registry::shortest_queue) so a job counts the instant it is
    /// placed, closing a thundering-herd window where several placements in a row would
    /// otherwise all see the same stale idle worker.
    inbox_len: AtomicUsize,
    inbox: Mutex<VecDeque<Job>>,
    waker: ReactorWaker,
    /// Never registered with a real source — a worker drains its inbox unconditionally
    /// every loop iteration once woken, so which token fired never matters.
    inbox_token: Token,
}

impl WorkerHandle {
    fn new(reactor: &mut Reactor) -> Self {
        WorkerHandle {
            ready_len: AtomicUsize::new(0),
            inbox_len: AtomicUsize::new(0),
            inbox: Mutex::new(VecDeque::new()),
            waker: reactor.helper_waker(),
            inbox_token: reactor.alloc_token(),
        }
    }

    fn wake(&self) {
        self.waker.complete(self.inbox_token);
    }
}

/// Every worker of one `scheduler::run` call, reached by index. `0` is the leader, publishes
/// itself at startup; the rest start lazily (see the module doc). Also the shared home for a
/// `Server`'s table and the `Connection`/`Server` id counter (`net::server`'s own
/// ownership-rule comment).
pub(crate) struct Registry {
    workers: Vec<OnceLock<WorkerHandle>>,
    shutdown: AtomicBool,
    servers: Mutex<HashMap<u64, Arc<ServerState>>>,
    next_handle: AtomicU64,
    /// Runs the helper-spawning closure exactly once, across however many fibers race to be
    /// first through [`shortest_queue`](Self::shortest_queue).
    helpers_started: OnceLock<()>,
    helpers: Mutex<Vec<JoinHandle<()>>>,
}

impl Registry {
    pub(crate) fn new(worker_count: usize) -> Self {
        assert!(worker_count > 0, "a registry needs at least one worker");
        Registry {
            workers: (0..worker_count).map(|_| OnceLock::new()).collect(),
            shutdown: AtomicBool::new(false),
            servers: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
            helpers_started: OnceLock::new(),
            helpers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// A fresh `Connection`/`Server` handle id, never reused, shared across every worker so
    /// two can never hand out the same one.
    pub(crate) fn next_handle(&self) -> u64 {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn servers(&self) -> &Mutex<HashMap<u64, Arc<ServerState>>> {
        &self.servers
    }

    /// Publish worker `index`'s own handle, built from its own reactor.
    pub(crate) fn publish(&self, index: usize, reactor: &mut Reactor) {
        let _ = self.workers[index].set(WorkerHandle::new(reactor));
    }

    /// Run `spawn` exactly once for this registry, however many fibers call this
    /// concurrently — see `scheduler::ensure_helpers_started`.
    pub(crate) fn start_helpers_once(&self, spawn: impl FnOnce()) {
        self.helpers_started.get_or_init(spawn);
    }

    pub(crate) fn set_helpers(&self, handles: Vec<JoinHandle<()>>) {
        *self
            .helpers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = handles;
    }

    pub(crate) fn take_helpers(&self) -> Vec<JoinHandle<()>> {
        std::mem::take(
            &mut *self
                .helpers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn handle(&self, index: usize) -> &WorkerHandle {
        self.workers[index]
            .get()
            .expect("shortest_queue/place/set_ready_len only run after ensure_helpers_started")
    }

    /// Record worker `index`'s current run-queue length, for [`shortest_queue`](Self::shortest_queue)
    /// to read. Called only by worker `index`'s own thread.
    pub(crate) fn set_ready_len(&self, index: usize, len: usize) {
        self.handle(index).ready_len.store(len, Ordering::Relaxed);
    }

    /// The worker with the shortest run queue right now — counting a job already placed but
    /// not yet drained the same as one actually running, so several placement decisions in
    /// quick succession spread out instead of piling onto the same momentarily-stale-looking
    /// worker (see [`WorkerHandle::inbox_len`]'s own doc). Ties to the lowest index.
    pub(crate) fn shortest_queue(&self) -> usize {
        (0..self.workers.len())
            .map(|index| {
                let handle = self.handle(index);
                let len = handle.ready_len.load(Ordering::Relaxed)
                    + handle.inbox_len.load(Ordering::Relaxed);
                (len, index)
            })
            .min()
            .map(|(_len, index)| index)
            .expect("a registry always has at least one worker")
    }

    /// Hand `job` to worker `target`, to run on its own thread — safe to call from any
    /// thread, including `target`'s own (a launch may place itself on its own worker).
    pub(crate) fn place(&self, target: usize, job: Job) {
        let handle = self.handle(target);
        handle.inbox_len.fetch_add(1, Ordering::Relaxed);
        handle
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(job);
        handle.wake();
    }

    /// Take every job waiting in worker `index`'s own inbox, to run on its own thread. Called
    /// only by worker `index`'s own thread, every loop iteration.
    pub(crate) fn drain(&self, index: usize) -> VecDeque<Job> {
        let handle = self.handle(index);
        let jobs = std::mem::take(
            &mut *handle
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        handle.inbox_len.fetch_sub(jobs.len(), Ordering::Relaxed);
        jobs
    }

    /// Ask every started helper to stop once idle, and wake each one. A never-started slot
    /// (no launch ever placed anything) is skipped, not an error.
    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for slot in &self.workers {
            if let Some(handle) = slot.get() {
                handle.wake();
            }
        }
    }

    pub(crate) fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}
