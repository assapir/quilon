// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The runtime's blocking-call pool: run a call with no non-blocking form on a helper OS
//! thread, and park the calling fiber on a reactor token until the call's result is ready.
//! [`run_blocking`] is the one entry point; [`crate::net`]'s hostname resolution (`getaddrinfo`
//! has no non-blocking form) is its first caller.
//!
//! Sizing: a small set of BASE threads, started lazily on the first call and kept for the rest
//! of the process (reused across calls — a program that never makes one starts no threads),
//! plus OVERFLOW threads the pool grows by one at a time, up to [`BLOCKING_POOL_CEILING`]
//! threads in total, whenever a call arrives with more jobs in flight than the pool currently
//! has threads for. An overflow thread is temporary: it exits as soon as it finds the job queue
//! empty, so the pool shrinks back to its base once a burst of concurrent calls drains. Neither
//! the base nor the ceiling is configurable — no environment variable, no program-side knob.

use crate::scheduler::{park_on_readiness, register_helper_waker};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

/// One queued unit of work, fully type-erased: it already carries everything it needs to
/// deliver its own result and wake its own fiber, so the pool itself never has to know the
/// caller's result type.
type Job = Box<dyn FnOnce() + Send>;

/// Lower bound on [`BlockingPool`]'s base thread count, regardless of CPU count — a
/// single-core machine still gets a few concurrent blocking calls going.
const BLOCKING_POOL_BASE_MINIMUM: usize = 4;

/// Upper bound on [`BlockingPool`]'s total thread count (base threads plus every thread it
/// grows to under load). Not configurable — named here, never read from the environment — so
/// a call that arrives once the pool is already this large waits in the queue instead of
/// growing it further.
const BLOCKING_POOL_CEILING: usize = 16;

/// Run `job` on the runtime's blocking-call pool, and park the calling fiber on a fresh
/// reactor token until a worker delivers its result. The call runs on the pool's thread while
/// the fiber that asked for it is the only thing suspended — every other fiber, timer, and
/// socket on the scheduler keeps making progress.
///
/// `Err` only when the pool could not start (thread creation refused) or `job` panicked —
/// never for a value `job` itself computes, which flows back as `Ok` whatever it is (callers
/// whose own work is fallible, like a DNS lookup, use `T = io::Result<_>` and flatten it).
pub(crate) fn run_blocking<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> io::Result<T> {
    let pool = blocking_pool()?;
    run_blocking_on(&pool, job)
}

/// [`run_blocking`]'s implementation, taking `pool` explicitly so a test can drive an isolated
/// pool directly rather than the lazily started global one.
fn run_blocking_on<T: Send + 'static>(
    pool: &Arc<BlockingPool>,
    job: impl FnOnce() -> T + Send + 'static,
) -> io::Result<T> {
    let (token, waker) = register_helper_waker();
    let (result_sender, result_receiver) = mpsc::channel::<T>();
    let boxed: Job = Box::new(move || {
        // Caught rather than left to unwind off the end of the thread: an uncaught panic here
        // would never send a result or wake the reactor, leaving the parked fiber stuck
        // forever. A panic sends nothing, so the `recv` below sees a disconnected sender and
        // reports it as this call's `Err`.
        if let Ok(value) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
            let _ = result_sender.send(value);
        }
        waker.complete(token);
    });
    submit(pool, boxed);
    park_on_readiness(token);
    result_receiver
        .recv()
        .map_err(|_| io::Error::other("the blocking-call pool's worker panicked"))
}

/// The lazily started global [`BlockingPool`]: absent until the first blocking call. A
/// `Result` cached here (not a bare pool) so a base-thread spawn failure the first time (thread
/// creation refused) is remembered and answered the same way on every later call — as an
/// ordinary `Err`, never the process-aborting panic `thread::spawn` itself raises.
fn blocking_pool() -> io::Result<Arc<BlockingPool>> {
    static POOL: OnceLock<io::Result<Arc<BlockingPool>>> = OnceLock::new();
    match POOL.get_or_init(|| start_blocking_pool(blocking_pool_base_size())) {
        Ok(pool) => Ok(Arc::clone(pool)),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

/// The pool's base thread count: the process's CPU count — honouring cgroup quotas and
/// affinity masks, via [`std::thread::available_parallelism`] — clamped to
/// [`BLOCKING_POOL_BASE_MINIMUM`]..=[`BLOCKING_POOL_CEILING`].
fn blocking_pool_base_size() -> usize {
    let cpu_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    cpu_count.clamp(BLOCKING_POOL_BASE_MINIMUM, BLOCKING_POOL_CEILING)
}

struct BlockingPool {
    queue: JobQueue,
    state: Mutex<PoolState>,
}

/// The pool's shared job queue: a plain `VecDeque` behind a `Mutex`, with a `Condvar` a BASE
/// worker's blocking wait ([`pop_blocking`](Self::pop_blocking)) parks on. Not an
/// `mpsc::Receiver` shared behind a `Mutex`: `Receiver::recv` blocks WHILE holding whatever
/// lock guards it, so with more than one blocking consumer the first to call it would starve
/// every other base thread — and every overflow thread's non-blocking check — out of the
/// queue for good. `Condvar::wait` releases the mutex while parked, which is exactly what
/// avoids that.
struct JobQueue {
    jobs: Mutex<VecDeque<Job>>,
    condvar: Condvar,
}

impl JobQueue {
    fn new() -> Self {
        JobQueue {
            jobs: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        }
    }

    fn push(&self, job: Job) {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(job);
        // Exactly one job arrived, so waking one waiter (rather than every base thread) is
        // enough for it to be picked up.
        self.condvar.notify_one();
    }

    /// A base worker's wait: blocks until a job is available. `Condvar::wait` releases `jobs`'
    /// lock for the whole time this thread is parked, so other threads keep working the queue.
    fn pop_blocking(&self) -> Job {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(job) = jobs.pop_front() {
                return job;
            }
            jobs = self
                .condvar
                .wait(jobs)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// An overflow worker's check: `None` if the queue is empty right now, with no wait.
    fn pop_nonblocking(&self) -> Option<Job> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }
}

/// [`BlockingPool`]'s bookkeeping for its own growth decisions: how many threads it has right
/// now, and how many jobs are in flight (queued or running) — comparing the two, at every
/// submission, is what decides whether to grow (see [`submit`]).
struct PoolState {
    total_threads: usize,
    in_flight: usize,
}

/// Start a [`BlockingPool`] with `base_size` base worker threads, or the first thread-creation
/// failure. Split out from [`blocking_pool`] so a test can force a small base size rather than
/// the real CPU count.
fn start_blocking_pool(base_size: usize) -> io::Result<Arc<BlockingPool>> {
    let pool = Arc::new(BlockingPool {
        queue: JobQueue::new(),
        state: Mutex::new(PoolState {
            total_threads: 0,
            in_flight: 0,
        }),
    });
    for _ in 0..base_size {
        spawn_base_worker(&pool)?;
    }
    Ok(pool)
}

/// Spawn one base worker (a permanent thread, looping on [`run_worker`] for the pool's whole
/// life) and count it, or propagate the thread-creation failure without counting it.
fn spawn_base_worker(pool: &Arc<BlockingPool>) -> io::Result<()> {
    let worker_pool = Arc::clone(pool);
    std::thread::Builder::new().spawn(move || run_worker(&worker_pool, true))?;
    pool.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .total_threads += 1;
    Ok(())
}

/// A worker's whole life: repeatedly run the next job. A BASE worker (`is_base`) blocks for
/// it and never exits; an OVERFLOW worker pulls without blocking and exits — uncounting
/// itself — the moment it finds none waiting, which is what shrinks the pool back to its base
/// once a burst of concurrent calls drains.
fn run_worker(pool: &BlockingPool, is_base: bool) {
    loop {
        let job = if is_base {
            Some(pool.queue.pop_blocking())
        } else {
            pool.queue.pop_nonblocking()
        };
        let Some(job) = job else { break };
        job();
        pool.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .in_flight -= 1;
    }
    if !is_base {
        pool.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .total_threads -= 1;
    }
}

/// Queue `job` on `pool`, growing the pool by one overflow thread ([`run_worker`]) if
/// more jobs are now in flight than the pool has threads for — the pigeonhole case: some job,
/// this one or an earlier one, has no thread that could possibly be running it yet. Never grows
/// past [`BLOCKING_POOL_CEILING`]; beyond it, every job waits in the queue for an existing
/// thread to free up. Counting `in_flight` against `total_threads`, rather than watching which
/// threads happen to be idle, makes the decision immediate and race-free: it does not depend on
/// how quickly an OS thread wakes from its wait.
fn submit(pool: &Arc<BlockingPool>, job: Job) {
    let spawn_overflow = {
        let mut state = pool
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.in_flight += 1;
        let grow =
            state.in_flight > state.total_threads && state.total_threads < BLOCKING_POOL_CEILING;
        if grow {
            state.total_threads += 1;
        }
        grow
    };
    pool.queue.push(job);
    if spawn_overflow {
        let worker_pool = Arc::clone(pool);
        let spawned = std::thread::Builder::new().spawn(move || run_worker(&worker_pool, false));
        if spawned.is_err() {
            // The job is already queued for an existing thread to eventually pick up, so a
            // refused overflow thread isn't fatal — just undo the capacity bump counted above.
            pool.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .total_threads -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc_test_harness::on_gc_thread;
    use crate::scheduler::{run, spawn};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn a_blocking_call_parks_the_fiber_without_blocking_the_scheduler() {
        // The job hook blocks until a sibling fiber on the same scheduler has run and signaled
        // back over a channel. That rendezvous is only satisfiable if the scheduler thread
        // stays free to run the sibling while the job is in flight — exactly what running the
        // job on a helper thread buys. If the job ran synchronously on the fiber/scheduler
        // thread instead, the sibling would never get to run and this would deadlock;
        // `recv_timeout` turns that into a clear test failure instead of hanging the suite.
        static COMPLETED: AtomicBool = AtomicBool::new(false);
        COMPLETED.store(false, Ordering::SeqCst);

        let (sibling_ran_sender, sibling_ran_receiver) = mpsc::channel::<()>();

        on_gc_thread(move || {
            run(move || {
                spawn(move || {
                    let job = move || {
                        // Only satisfiable if the sibling below actually ran while this job was
                        // in flight — impossible if run_blocking ran it synchronously.
                        sibling_ran_receiver
                            .recv_timeout(Duration::from_secs(5))
                            .expect("sibling fiber never ran while the job was in flight");
                        42
                    };
                    let result = run_blocking(job);
                    assert_eq!(result.ok(), Some(42), "the job's own result came back");
                    COMPLETED.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let _ = sibling_ran_sender.send(());
                });
            });
        });

        assert!(COMPLETED.load(Ordering::SeqCst), "the call completed");
    }

    #[test]
    fn a_panicking_job_wakes_the_fiber_with_an_error_instead_of_hanging_it() {
        // A helper thread that panics before sending a result and waking the reactor would
        // otherwise leave the parked fiber stuck forever; `run_blocking` must catch that and
        // still resolve to an `Err`.
        static RESOLVED_TO_ERROR: AtomicBool = AtomicBool::new(false);
        RESOLVED_TO_ERROR.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let job = || -> i32 { panic!("a job that misbehaves") };
                    let result = run_blocking(job);
                    RESOLVED_TO_ERROR.store(result.is_err(), Ordering::SeqCst);
                });
            });
        });

        assert!(
            RESOLVED_TO_ERROR.load(Ordering::SeqCst),
            "a panicking job must resolve to an error, not hang the fiber"
        );
    }

    #[test]
    fn several_concurrent_blocking_calls_on_one_scheduler_all_resolve() {
        // More calls than the pool could ever have threads for at once (whatever this
        // machine's CPU count clamps the base to), so some are guaranteed to queue behind
        // others: proves the pool serves every fiber's call, not just as many as it has
        // threads for, and hands each fiber back its own job's result rather than another's.
        const CALLS: usize = BLOCKING_POOL_CEILING + 3;
        static RESOLVED_COUNT: AtomicUsize = AtomicUsize::new(0);
        RESOLVED_COUNT.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                for index in 0..CALLS {
                    spawn(move || {
                        let job = move || {
                            // A little work on the worker thread, so concurrent calls actually
                            // overlap rather than finishing one at a time.
                            std::thread::sleep(Duration::from_millis(10));
                            index
                        };
                        let result = run_blocking(job).expect("every concurrent call must resolve");
                        assert_eq!(
                            result, index,
                            "a fiber must get its own job's result back, not another's"
                        );
                        RESOLVED_COUNT.fetch_add(1, Ordering::SeqCst);
                    });
                }
            });
        });

        assert_eq!(RESOLVED_COUNT.load(Ordering::SeqCst), CALLS);
    }

    /// A manual-reset gate: every job below blocks on [`ReleaseGate::wait`] until the test's
    /// controller thread calls [`ReleaseGate::release`], so the test can hold a known number of
    /// jobs in flight at once before letting any of them finish.
    struct ReleaseGate {
        released: Mutex<bool>,
        condvar: Condvar,
    }

    impl ReleaseGate {
        fn new() -> Self {
            ReleaseGate {
                released: Mutex::new(false),
                condvar: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut released = self.released.lock().unwrap_or_else(|p| p.into_inner());
            while !*released {
                released = self
                    .condvar
                    .wait(released)
                    .unwrap_or_else(|p| p.into_inner());
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap_or_else(|p| p.into_inner()) = true;
            self.condvar.notify_all();
        }
    }

    #[test]
    fn the_pool_grows_past_its_base_under_load_and_stops_at_the_ceiling() {
        // An isolated pool with its base forced small, so growth is observable regardless of
        // this machine's real CPU count; BLOCKING_POOL_CEILING is still the real ceiling
        // constant, shared with production.
        const BASE: usize = 4;
        const CALLS: usize = BLOCKING_POOL_CEILING + 3;

        let pool = start_blocking_pool(BASE).expect("start an isolated pool for this test");
        let (started_sender, started_receiver) = mpsc::channel::<()>();
        let release_gate = Arc::new(ReleaseGate::new());

        static PEAK_THREADS: AtomicUsize = AtomicUsize::new(0);
        PEAK_THREADS.store(0, Ordering::SeqCst);

        // A real OS thread, concurrent with the scheduler's run() below (which is itself
        // synchronous and must not be the one waiting here): collect one "started" signal per
        // call that has actually begun running, sample the pool's thread count once enough
        // have started to prove the ceiling was reached, then release every blocked call so
        // run() can finish.
        let controller_pool = Arc::clone(&pool);
        let controller_gate = Arc::clone(&release_gate);
        let controller = std::thread::spawn(move || {
            for _ in 0..BLOCKING_POOL_CEILING {
                started_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("a call started");
            }
            // A short settle so a growth decision already in flight has finished before the
            // sample below reads it.
            std::thread::sleep(Duration::from_millis(50));
            PEAK_THREADS.store(
                controller_pool
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .total_threads,
                Ordering::SeqCst,
            );
            controller_gate.release();
            // Drain the rest of the "started" signals so every call has a chance to run.
            for _ in BLOCKING_POOL_CEILING..CALLS {
                let _ = started_receiver.recv_timeout(Duration::from_secs(5));
            }
        });

        let scheduler_pool = Arc::clone(&pool);
        on_gc_thread(move || {
            run(move || {
                for _ in 0..CALLS {
                    let pool = Arc::clone(&scheduler_pool);
                    let started_sender = started_sender.clone();
                    let release_gate = Arc::clone(&release_gate);
                    spawn(move || {
                        let job = move || {
                            let _ = started_sender.send(());
                            release_gate.wait();
                            0
                        };
                        let _ = run_blocking_on(&pool, job);
                    });
                }
            });
        });

        controller.join().expect("controller thread");
        assert_eq!(
            PEAK_THREADS.load(Ordering::SeqCst),
            BLOCKING_POOL_CEILING,
            "more concurrent calls than the ceiling must grow the pool all the way to it"
        );

        // Every overflow thread exits once it finds the queue empty, but on a loaded machine
        // that can take a while (12 threads all contending for the same job-queue mutex right
        // after release); poll rather than trust one fixed sleep.
        let deadline = Instant::now() + Duration::from_secs(5);
        let final_thread_count = loop {
            let count = pool
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .total_threads;
            if count == BASE || Instant::now() >= deadline {
                break count;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            final_thread_count, BASE,
            "every overflow thread must have exited, leaving only the base"
        );
    }
}
