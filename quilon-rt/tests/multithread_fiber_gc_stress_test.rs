// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! SPIKE evidence: can Quilon fibers run on more than one OS thread under the Boehm GC,
//! fibers pinned to whichever thread spawned them, and survive sustained collections? See
//! `quilon-rt/src/gc.rs`'s module doc ("Running fibers on more than one OS thread") for the
//! two scanning changes this exercises: fiber-side switch ordering
//! (`gc::enter_fiber`/`gc::leave_fiber`) and the lock-free per-thread scan-state registry.
//!
//! Every test here is `#[ignore]`d — a multi-minute stress run has no place in the normal
//! suite or CI — and run explicitly:
//!
//! ```text
//! cargo test -p quilon-rt --test multithread_fiber_gc_stress_test -- --ignored --nocapture
//! ```
//!
//! Each spawns `WORKER_THREADS` OS threads, each registered with the collector the same
//! way `quilon-rt/src/blocking.rs`'s pool threads are (`quilon_rt::register_thread`), each
//! running its OWN [`quilon_rt::scheduler::run`] instance with `FIBERS_PER_WORKER` fibers.
//! Every fiber loops for the run's whole duration: build a small linked list on the GC heap
//! (`quilon_rt::__alloc`, so it is scanned like any Quilon-allocated record), sized
//! differently every iteration, held only by this fiber's own stack locals (never a global,
//! never a plain Rust `Box`); sleep briefly, parking the fiber and letting the scheduler
//! resume a sibling; then walk the list and compare its checksum against what it built,
//! failing loudly on any mismatch. A dedicated thread calls `GC_gcollect` in a tight loop
//! for the same duration, so every fiber switch on every worker thread races a collection.
//!
//! A checksum mismatch, a panic, or the process not returning inside a generous multiple of
//! the requested duration (this harness leaves that last one to whoever runs it — a
//! genuinely hung scheduler thread has no clean way to time out from the outside without
//! `pthread_cancel`, which is not something this crate reaches for) is a failure of the
//! scheme; see the module doc in `gc.rs` and the PR body for what a failure here would mean
//! for the M:N scheduler this spike is evidence for or against.

use quilon_rt::scheduler::{run, sleep, spawn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// Linked into the final test binary the same way `quilon-rt/src/scheduler.rs`'s own unit
// tests reach it: `libgc` travels with the crate's rlib via its own `#[link]` attribute, so
// re-declaring the one extra symbol this test needs is enough — no new build-script wiring.
#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_gcollect();
}

/// One GC-heap node: `next` is a real pointer the collector must trace, so a wrongly-freed
/// or wrongly-unscanned chain would corrupt `value` or `next` under it, not just leak.
#[repr(C)]
struct Node {
    value: u64,
    next: *mut Node,
}

/// Build a `len`-node chain from freshly GC-allocated nodes, values derived from `seed` so
/// [`expected_checksum`] can recompute what [`checksum`] should find without keeping a
/// second copy of the list around.
fn build_chain(len: usize, seed: u64) -> *mut Node {
    let mut head: *mut Node = std::ptr::null_mut();
    for i in 0..len {
        let raw = quilon_rt::__alloc(std::mem::size_of::<Node>() as i64) as *mut Node;
        assert!(!raw.is_null(), "the collector must never hand back null");
        // SAFETY: `raw` is a fresh, uniquely-owned allocation sized for one `Node`.
        unsafe {
            (*raw).value = seed.wrapping_add(i as u64);
            (*raw).next = head;
        }
        head = raw;
    }
    head
}

/// Sum every node's value by walking `next` — the read that would see a torn or
/// wrongly-freed node first, since it dereferences every pointer in the chain.
fn checksum(mut node: *mut Node) -> u64 {
    let mut sum = 0u64;
    while !node.is_null() {
        // SAFETY: every node in a chain this module built is either live GC memory this
        // fiber alone holds the only reachable path to, or has already been proven
        // reachable by the previous iteration finding it via `next`.
        unsafe {
            sum = sum.wrapping_add((*node).value);
            node = (*node).next;
        }
    }
    sum
}

fn expected_checksum(len: usize, seed: u64) -> u64 {
    (0..len).fold(0u64, |sum, i| sum.wrapping_add(seed.wrapping_add(i as u64)))
}

/// Shared, cross-thread evidence: what the PR body reports as "collections performed,
/// fibers switched, allocations", plus the one-shot latch a mismatch trips so every worker
/// notices and stops promptly instead of grinding on for the rest of the run.
struct Evidence {
    collections: AtomicU64,
    resume_cycles: AtomicU64,
    node_allocations: AtomicU64,
    failed: AtomicBool,
}

impl Evidence {
    fn new() -> Self {
        Evidence {
            collections: AtomicU64::new(0),
            resume_cycles: AtomicU64::new(0),
            node_allocations: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        }
    }
}

/// A cheap, deterministic-per-fiber PRNG (splitmix64) — no new dependency for what is
/// only ever used to vary this fiber's own chain length and sleep duration a little.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// One worker OS thread's whole life: register with the collector exactly as
/// `blocking.rs`'s pool threads do, then run its OWN scheduler with `fibers_per_worker`
/// fibers, each looping until `deadline` (or the shared [`Evidence::failed`] latch trips).
fn run_worker_thread(evidence: Arc<Evidence>, fibers_per_worker: usize, deadline: Instant) {
    let _gc_thread = quilon_rt::register_thread();
    run(move || {
        for fiber_index in 0..fibers_per_worker {
            let evidence = Arc::clone(&evidence);
            spawn(move || {
                let mut random_state = fiber_index as u64 + 1;
                while Instant::now() < deadline && !evidence.failed.load(Ordering::Relaxed) {
                    let draw = next_random(&mut random_state);
                    let len = 4 + (draw % 32) as usize;
                    let seed = draw;

                    let head = build_chain(len, seed);
                    evidence
                        .node_allocations
                        .fetch_add(len as u64, Ordering::Relaxed);

                    // Park and resume often — the one place a fiber switch races a
                    // concurrent collection on this thread.
                    sleep(Duration::from_micros(200 + (draw % 800)));

                    let got = checksum(head);
                    let want = expected_checksum(len, seed);
                    if got != want {
                        evidence.failed.store(true, Ordering::SeqCst);
                        panic!(
                            "checksum mismatch after resume: fiber {fiber_index} built a \
                             {len}-node chain (seed {seed}), expected checksum {want}, got \
                             {got} — a collection during the switch lost or corrupted this \
                             fiber's stack-held roots"
                        );
                    }
                    evidence.resume_cycles.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
}

/// The collector thread: `GC_gcollect` in a tight loop for the run's whole duration, the
/// same way every worker's fiber switches are meant to survive racing it.
fn run_collector_thread(evidence: Arc<Evidence>, deadline: Instant) {
    let _gc_thread = quilon_rt::register_thread();
    while Instant::now() < deadline && !evidence.failed.load(Ordering::Relaxed) {
        // SAFETY: a plain, reentrant-safe collector call; this thread is registered above.
        unsafe { GC_gcollect() };
        evidence.collections.fetch_add(1, Ordering::Relaxed);
    }
}

fn run_stress(worker_threads: usize, fibers_per_worker: usize, duration: Duration) {
    let evidence = Arc::new(Evidence::new());
    let deadline = Instant::now() + duration;

    let workers: Vec<_> = (0..worker_threads)
        .map(|_| {
            let evidence = Arc::clone(&evidence);
            std::thread::spawn(move || run_worker_thread(evidence, fibers_per_worker, deadline))
        })
        .collect();
    let collector = {
        let evidence = Arc::clone(&evidence);
        std::thread::spawn(move || run_collector_thread(evidence, deadline))
    };

    for worker in workers {
        worker
            .join()
            .expect("a worker thread panicked — see the checksum-mismatch message above");
    }
    collector.join().expect("the collector thread panicked");

    let failed = evidence.failed.load(Ordering::SeqCst);
    let collections = evidence.collections.load(Ordering::Relaxed);
    let resume_cycles = evidence.resume_cycles.load(Ordering::Relaxed);
    let node_allocations = evidence.node_allocations.load(Ordering::Relaxed);
    eprintln!(
        "stress run: {worker_threads} worker thread(s) x {fibers_per_worker} fibers, \
         {duration:?} requested — {collections} collections, {resume_cycles} fiber \
         resume/verify cycles, {node_allocations} node allocations, failed={failed}"
    );
    assert!(
        !failed,
        "a fiber's checksum mismatched after a resume — see the panic message above"
    );
}

#[test]
#[ignore = "multi-minute stress run; execute explicitly with --ignored"]
fn two_threads_survive_ten_minutes_of_sustained_collections() {
    run_stress(2, 200, Duration::from_secs(10 * 60));
}

#[test]
#[ignore = "multi-minute stress run; execute explicitly with --ignored"]
fn four_threads_survive_ten_minutes_of_sustained_collections() {
    run_stress(4, 200, Duration::from_secs(10 * 60));
}

#[test]
#[ignore = "final evidence: a 30-minute run; execute explicitly with --ignored"]
fn four_threads_survive_thirty_minutes_of_sustained_collections() {
    run_stress(4, 200, Duration::from_secs(30 * 60));
}

/// Single-threaded microbenchmark: what design C's per-switch `gc::do_blocking` costs
/// relative to `main`'s scheme, isolated from every other change — one fiber, one thread,
/// no collector contention. Not an assertion: run manually (three times, by hand, on each
/// side) and compare the printed medians; see the PR body for the numbers this produced.
#[test]
#[ignore = "microbenchmark; compare against the same test built on main, see the PR body"]
fn microbench_100k_fiber_switches() {
    let _gc_thread = quilon_rt::register_thread();
    let start = std::time::Instant::now();
    run(|| {
        spawn(|| {
            for _ in 0..100_000 {
                sleep(Duration::from_nanos(0));
            }
        });
    });
    eprintln!("100k fiber switches: {:?}", start.elapsed());
}

/// Single-threaded microbenchmark: what design C's per-allocation `gc::with_gc_active`
/// costs relative to `main`'s bare `GC_malloc`/`GC_malloc_atomic` call, isolated the same
/// way as [`microbench_100k_fiber_switches`]. Not an assertion; see its own doc.
#[test]
#[ignore = "microbenchmark; compare against the same test built on main, see the PR body"]
fn microbench_1m_small_allocations() {
    let _gc_thread = quilon_rt::register_thread();
    let start = std::time::Instant::now();
    run(|| {
        spawn(|| {
            for _ in 0..1_000_000 {
                let p = quilon_rt::__alloc(32);
                std::hint::black_box(p);
            }
        });
    });
    eprintln!("1M small allocations: {:?}", start.elapsed());
}

/// Round 3 microbenchmark: design A's cost is per switch, and specifically under
/// contention — every switch anywhere in the process briefly holds `GC_dont_gc` above
/// zero (a single global counter), so the more concurrent switching, the more a collector
/// hammering `GC_gcollect` finds collection held off. Four worker threads each run their
/// own scheduler with one fiber doing 100k switches (`sleep(0)`, park/resume), while a
/// fifth thread calls `GC_gcollect` in a tight loop for the same duration. Wall time only
/// (not collections/switches — this is a timing comparison, not a survival one); run on
/// both the design-C and design-A commits, see the PR body for the numbers.
#[test]
#[ignore = "microbenchmark; run on both the design-C and design-A commits, see the PR body"]
fn microbench_4threads_100k_switches_under_collector_pressure() {
    let stop = Arc::new(AtomicBool::new(false));
    let collector = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let _gc_thread = quilon_rt::register_thread();
            while !stop.load(Ordering::Relaxed) {
                unsafe { GC_gcollect() };
            }
        })
    };

    let start = std::time::Instant::now();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let _gc_thread = quilon_rt::register_thread();
                run(|| {
                    spawn(|| {
                        for _ in 0..100_000 {
                            sleep(Duration::from_nanos(0));
                        }
                    });
                });
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a worker thread panicked");
    }
    let elapsed = start.elapsed();

    stop.store(true, Ordering::Relaxed);
    collector.join().expect("the collector thread panicked");

    eprintln!("4 threads x 100k switches under collector pressure: {elapsed:?}");
}
