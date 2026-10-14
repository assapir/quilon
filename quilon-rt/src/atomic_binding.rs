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
//! The code generator picks the binding's own lock by hashing its NAME to an index into a
//! small, fixed table (`CodeGenerator::atomic_lock_id`, computed at COMPILE time and baked
//! into every `__atomic_begin`/`__atomic_end` call as the literal `id`) — no per-program
//! registration here, no runtime sizing. Two bindings whose names hash to the same slot
//! share a lock; that only ever over-serializes (harmless) and never under-serializes
//! (which would be the actual bug), so a collision costs a little contention, never
//! correctness. The calls wrap the lowered reassignment
//! (`CodeGenerator::generate_variable_declaration`) and the lowered read of an atomic
//! binding's identifier (`CodeGenerator::generate_expression_inner`) alike.
//!
//! The checker already guarantees an atomic reassignment's right side never parks (it is a
//! compile error otherwise — see `src/deferral.rs`), so this is always a short, bounded
//! critical section: a spin lock, not a `Mutex`, since there is never a reason to sleep
//! waiting for one to release. On a single-worker program every acquire finds its lock free
//! on the first try (nothing else runs between `begin` and `end`), so the cost there is the
//! two atomic instructions themselves, paid only at atomic-binding sites — a program with
//! none emits no calls here at all.

use std::sync::atomic::{AtomicBool, Ordering};

/// How many locks the table holds. Deliberately generous relative to how many atomic
/// bindings a real program declares (each is hand-written in source), so two unrelated
/// bindings sharing a lock is a rare, harmless coincidence rather than the common case.
const LOCK_COUNT: usize = 251; // prime, so a short/repetitive name does not alias in a pattern

static LOCKS: [AtomicBool; LOCK_COUNT] = {
    // The repeat-expression idiom for a non-`Copy` const: `FALSE` is never read back as a
    // shared constant value anywhere else, only expanded once into this one array literal,
    // so there is no risk of the aliasing clippy's lint otherwise guards against.
    #[allow(clippy::declare_interior_mutable_const)]
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; LOCK_COUNT]
};

fn slot(id: i64) -> &'static AtomicBool {
    &LOCKS[(id as u64 as usize) % LOCK_COUNT]
}

/// Acquire the lock for atomic binding `id` (a hash of its name, computed at compile time —
/// see the module doc). Spins rather than blocks: the critical section it opens is always
/// short and never parks (the checker enforces that for a reassignment's right side), so
/// there is nothing to gain from a syscall-backed `Mutex` here.
#[unsafe(no_mangle)]
pub extern "C" fn __atomic_begin(id: i64) {
    let lock = slot(id);
    while lock.swap(true, Ordering::Acquire) {
        std::hint::spin_loop();
    }
}

/// Release the lock [`__atomic_begin`] acquired for the same `id`.
#[unsafe(no_mangle)]
pub extern "C" fn __atomic_end(id: i64) {
    slot(id).store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as O};
    use std::thread;

    #[test]
    fn begin_excludes_a_concurrent_begin_on_the_same_id() {
        // A naive, unprotected `counter += 1` loses updates under real concurrency; wrapped
        // in begin/end on the SAME id, every increment from every thread survives.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        const THREADS: usize = 8;
        const PER_THREAD: u64 = 20_000;
        let id: i64 = 1;

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        __atomic_begin(id);
                        let current = COUNTER.load(O::Relaxed);
                        COUNTER.store(current + 1, O::Relaxed);
                        __atomic_end(id);
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
    fn different_ids_do_not_exclude_each_other() {
        // Two distinct locks must not serialize unrelated bindings: each thread alone can
        // make progress on the slot nothing else is touching.
        let a: i64 = 7;
        let b: i64 = 8;
        assert_ne!(
            slot(a) as *const AtomicBool,
            slot(b) as *const AtomicBool,
            "chose two ids whose slots collide — pick different fixture ids"
        );

        __atomic_begin(a);
        __atomic_begin(b); // would deadlock if `a` and `b` shared a slot
        __atomic_end(b);
        __atomic_end(a);
    }
}
