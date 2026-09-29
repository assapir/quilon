// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Cross-worker placement: the shortest-run-queue pick for a new launch or an accepted
//! connection, and the inbox each worker's own thread drains to receive one.
//!
//! One [`Registry`] is built per `scheduler::run` call and shared (`Arc`) by every worker
//! thread of that run — it never outlives the `run` call that built it, so two independent
//! `run` invocations in the same process (the test suite's own pattern, one dedicated GC
//! thread per test) never see each other's workers. Every worker publishes its own
//! [`WorkerHandle`] into the registry once, at startup, before any worker places work on
//! any other — [`Registry::publish`]'s caller is responsible for that ordering (a startup
//! barrier; see `scheduler::run`).
//!
//! Placement itself is one atomic read per worker ([`Registry::shortest_queue`]) — no lock,
//! no contention between placement decisions on different threads. Handing work to the
//! chosen worker ([`Registry::place`]) queues a boxed job in that worker's own inbox and
//! wakes its reactor through the same [`ReactorWaker`] mechanism a blocking-pool job's
//! completion already uses — the receiving worker drains its inbox on its own thread, every
//! loop iteration, so which exact token woke it never matters.

use crate::net::server::ServerState;
use crate::reactor::{Reactor, ReactorWaker};
use mio::Token;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A unit of work handed to a worker other than the one that created it. Already carries
/// everything it needs — the receiving worker's own thread just calls it.
pub(crate) type Job = Box<dyn FnOnce() + Send>;

struct WorkerHandle {
    /// This worker's current run-queue length, kept in step by `scheduler`'s own ready-queue
    /// push/pop points. Placement reads every worker's count and picks the minimum.
    ready_len: AtomicUsize,
    inbox: Mutex<VecDeque<Job>>,
    waker: ReactorWaker,
    /// The token `waker.complete` is called with on every wake — never registered with any
    /// real source. A worker drains its inbox unconditionally on every loop iteration once
    /// woken, so which exact token fired never matters, only that one did.
    inbox_token: Token,
}

impl WorkerHandle {
    fn new(reactor: &mut Reactor) -> Self {
        WorkerHandle {
            ready_len: AtomicUsize::new(0),
            inbox: Mutex::new(VecDeque::new()),
            waker: reactor.helper_waker(),
            inbox_token: reactor.alloc_token(),
        }
    }

    fn wake(&self) {
        self.waker.complete(self.inbox_token);
    }
}

/// Every worker of one `scheduler::run` call, reached by index. `0` is always the leader
/// (the worker `^`/`main` runs on). Also the one process-wide-for-this-run home for state a
/// `Server`/`Connection` handle needs reachable from more than one worker: the server table
/// itself (a `Server` handle created on one worker's fiber is killed from whichever worker
/// calls `Server.kill`, per `net::server`'s own ownership-rule comment) and the counter that
/// hands out every `Connection`/`Server` handle id, so two workers can never hand out the
/// same one.
pub(crate) struct Registry {
    workers: Vec<OnceLock<WorkerHandle>>,
    shutdown: AtomicBool,
    servers: Mutex<HashMap<u64, Arc<ServerState>>>,
    next_handle: AtomicU64,
}

impl Registry {
    pub(crate) fn new(worker_count: usize) -> Self {
        assert!(worker_count > 0, "a registry needs at least one worker");
        Registry {
            workers: (0..worker_count).map(|_| OnceLock::new()).collect(),
            shutdown: AtomicBool::new(false),
            servers: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }

    /// A fresh `Connection`/`Server` handle id, never reused and never handed out twice —
    /// one counter for every worker of this run, so a stale handle (one a program held onto
    /// past its own close/kill) can never be confused with a later, unrelated one, and two
    /// workers minting ids at once can never collide.
    pub(crate) fn next_handle(&self) -> u64 {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn servers(&self) -> &Mutex<HashMap<u64, Arc<ServerState>>> {
        &self.servers
    }

    /// Publish worker `index`'s own handle, built from its own reactor (its inbox token and
    /// waker come from there). Called once per worker, from that worker's own thread, before
    /// the startup barrier every worker waits on releases.
    pub(crate) fn publish(&self, index: usize, reactor: &mut Reactor) {
        let _ = self.workers[index].set(WorkerHandle::new(reactor));
    }

    fn handle(&self, index: usize) -> &WorkerHandle {
        self.workers[index]
            .get()
            .expect("every worker publishes its handle before the startup barrier releases it")
    }

    /// Record worker `index`'s current run-queue length, for [`shortest_queue`](Self::shortest_queue)
    /// to read. Called only by worker `index`'s own thread.
    pub(crate) fn set_ready_len(&self, index: usize, len: usize) {
        self.handle(index).ready_len.store(len, Ordering::Relaxed);
    }

    /// The worker with the shortest run queue right now, ties to the lowest index.
    pub(crate) fn shortest_queue(&self) -> usize {
        (0..self.workers.len())
            .map(|index| (self.handle(index).ready_len.load(Ordering::Relaxed), index))
            .min()
            .map(|(_len, index)| index)
            .expect("a registry always has at least one worker")
    }

    /// Hand `job` to worker `target`, to run on its own thread — safe to call from any
    /// thread, including `target`'s own (a launch may place itself on its own worker).
    pub(crate) fn place(&self, target: usize, job: Job) {
        let handle = self.handle(target);
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
        std::mem::take(
            &mut *self
                .handle(index)
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Ask every helper worker to stop once idle, and wake each one so a worker blocked in
    /// its own reactor wait notices promptly rather than on its next unrelated wake. Called
    /// once, by the leader, after its own loop has ended.
    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for index in 0..self.workers.len() {
            self.handle(index).wake();
        }
    }

    pub(crate) fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}
