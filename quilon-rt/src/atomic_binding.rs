// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0
//
// Quilon runtime library (`quilon-rt`). Copyright (C) 2026 Assaf Sapir.
//
// This crate is free software licensed under version 2 of the GNU General
// Public License (see LICENSE.md), WITH the Quilon runtime-library exception —
// a Classpath-style linking exception (see LICENSE-EXCEPTION.md). The exception
// means that programs you compile with Quilon, into which this runtime is
// linked or embedded, are NOT placed under the GPL by that linking and may be
// licensed under any terms. It frees only the compiled output: this crate's own
// source remains GPLv2, so a fork of `quilon-rt` stays GPLv2.

//! The critical section behind an atomic binding (`@name := …`, `docs/concurrency/README.md`
//! #sharing-state-across-fibers): makes a reassignment's whole right-side evaluation, and a
//! plain read, run as one step with respect to every other worker.
//!
//! One process-wide spin lock covers EVERY atomic binding — not a lock per binding. A
//! reassignment's right side may read a different atomic binding (`a := b + 1`), so a
//! per-binding scheme can deadlock: one worker's `a := b + 1` holds `a`'s lock wanting
//! `b`'s, while another's `b := a + 1` holds `b`'s wanting `a`'s, at the same instant — and
//! since the nesting is dynamic (the right side may call a function that reads a third
//! binding reached who knows how), no fixed lock ordering closes that off. A single lock
//! has no ordering to violate: every atomic reassignment in the whole process serializes on
//! it, one at a time, however many bindings exist.
//!
//! That single lock is made RE-ENTRANT with a per-thread (per-worker) depth counter, so a
//! read of ANY atomic binding — the same one or a different one — nested inside a
//! reassignment's own right-side evaluation does not try to re-acquire a lock this very
//! thread already holds: `__atomic_begin` takes the lock only when this thread's depth is
//! zero, then increments it either way; `__atomic_end` decrements it and releases the lock
//! only once it is back to zero. The code generator still gates every call site on whether
//! the name involved is actually a known atomic binding
//! (`CodeGenerator::atomic_bindings`/`should_lock_atomic_binding`), so this pair wraps the
//! lowered reassignment (`CodeGenerator::generate_variable_declaration` — the whole right
//! side plus the store) and the lowered read
//! (`CodeGenerator::generate_expression_inner`) alike, and a program with no atomic
//! bindings emits no calls here at all.
//!
//! The checker already guarantees an atomic reassignment's right side never parks (it is a
//! compile error otherwise — see `src/deferral.rs`), so the region this lock covers is
//! always short and never blocks on I/O: a spin lock, not a `Mutex`, since there is never a
//! reason to sleep waiting for one to release. On a single-worker program every acquire
//! finds the lock free (nothing else runs between `begin` and `end`), so the cost there is
//! the depth check and the atomic instructions themselves, paid only at atomic-binding
//! sites — a program with none pays nothing. The cost under real multi-worker contention is
//! that every atomic reassignment and read in the process, across every binding, now shares
//! one lock rather than spreading over several.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

static LOCK: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// How many `__atomic_begin` calls THIS thread is nested inside right now, with no
    /// matching `__atomic_end` yet. Zero outside of any; `__atomic_begin` acquires `LOCK`
    /// only on the 0 -> 1 transition, and `__atomic_end` releases it only on 1 -> 0 — so a
    /// read nested inside a reassignment's own right side (of the same binding or a
    /// different one) just counts up and down without re-acquiring a lock this thread
    /// already holds.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Enter an atomic-binding critical section. Spins rather than blocks: the section it opens
/// is always short and never parks (the checker enforces that for a reassignment's right
/// side), so there is nothing to gain from a syscall-backed `Mutex` here.
#[unsafe(no_mangle)]
pub extern "C" fn __atomic_begin() {
    DEPTH.with(|depth| {
        if depth.get() == 0 {
            while LOCK.swap(true, Ordering::Acquire) {
                std::hint::spin_loop();
            }
        }
        depth.set(depth.get() + 1);
    });
}

/// Leave the critical section [`__atomic_begin`] entered, on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn __atomic_end() {
    DEPTH.with(|depth| {
        let next = depth.get() - 1;
        depth.set(next);
        if next == 0 {
            LOCK.store(false, Ordering::Release);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as O};
    use std::thread;

    #[test]
    fn begin_excludes_a_concurrent_begin() {
        // A naive, unprotected `counter += 1` loses updates under real concurrency; wrapped
        // in begin/end, every increment from every thread survives.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        const THREADS: usize = 8;
        const PER_THREAD: u64 = 20_000;

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                thread::spawn(|| {
                    for _ in 0..PER_THREAD {
                        __atomic_begin();
                        let current = COUNTER.load(O::Relaxed);
                        COUNTER.store(current + 1, O::Relaxed);
                        __atomic_end();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(COUNTER.load(O::SeqCst), THREADS as u64 * PER_THREAD);
    }

    #[test]
    fn nested_begin_on_the_same_thread_does_not_self_deadlock() {
        // The read-modify-write case (`a := b + 1`, or a bare read nested inside another
        // binding's reassignment): a SECOND `__atomic_begin` on the thread already holding
        // the lock must not block — it only counts the nesting depth up.
        __atomic_begin();
        __atomic_begin(); // would hang forever on a non-reentrant lock
        __atomic_end();
        __atomic_end();
    }
}
