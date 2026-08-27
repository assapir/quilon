// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Cargo build script for `quilon-rt`: compiles the Boehm collector from the
//! `vendor/bdwgc` submodule into a static `libgc` and links it into this crate.
//!
//! Why build it here rather than link the system `-lgc`: `quilon build` links a
//! compiled program against `libquilon_rt.a`, and rustc *bundles* a `static`
//! native library's objects into a staticlib. So building the collector here puts
//! it inside `libquilon_rt.a` — which the compiler already embeds — and a produced
//! executable carries its own GC instead of needing `libgc` installed wherever it
//! runs. The same objects reach the `quilon` binary through the rlib, so the
//! in-process JIT resolves `GC_*` at the addresses it always did.
//!
//! The build is a single translation unit: upstream's `extra/gc.c` `#include`s
//! every collector source, which is exactly the "one link object" path bdwgc
//! documents for embedding. That keeps this to the `cc` crate — no autotools, no
//! cmake, no `libatomic_ops` (`GC_BUILTIN_ATOMIC` uses compiler intrinsics).

use std::path::Path;

/// Upstream's configure defaults for a threaded POSIX build — what the system
/// `libgc` this replaces was built with, so the collector behaves as it always
/// has. Kept in one list so a deviation from upstream's defaults is one visible
/// line. (The runtime is POSIX-only, so there is no Windows branch to pick.)
const GC_DEFINES: &[&str] = &[
    // Threads: the collector must know about threads the JIT host registers.
    "GC_THREADS",
    "_REENTRANT",
    // Compiler atomic intrinsics instead of a libatomic_ops dependency.
    "GC_BUILTIN_ATOMIC",
    // A pointer into the middle of an object keeps it alive. Quilon's `Text`/array
    // values are `{ ptr, len }` pairs whose `ptr` may be interior, so this one is
    // load bearing, not merely an upstream default.
    "ALL_INTERIOR_POINTERS",
    "NO_EXECUTE_PERMISSION",
    "USE_MMAP",
    "USE_MUNMAP",
    "HANDLE_FORK",
    "PARALLEL_MARK",
    "THREAD_LOCAL_ALLOC",
];

fn main() {
    let vendor = Path::new("vendor/bdwgc");
    let single_translation_unit = vendor.join("extra/gc.c");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", vendor.display());

    // The collector is a git submodule, so a clone made without it — or a GitHub
    // source tarball, which omits submodules entirely — leaves this directory
    // empty. Say that in one line, instead of letting `cc` bury it under a wall of
    // missing-header errors.
    assert!(
        single_translation_unit.exists(),
        "the bundled Boehm GC is missing: {} not found.\n\
         Quilon carries bdwgc as a git submodule — fetch it with:\n\
         \n    git submodule update --init\n\
         \n(a downloaded source tarball has no submodules; clone the repository instead)",
        single_translation_unit.display()
    );

    let mut build = cc::Build::new();
    build
        .file(&single_translation_unit)
        .include(vendor.join("include"))
        .include(vendor)
        // The collector casts between object representations throughout, and
        // upstream's own build documents this flag as required.
        .flag_if_supported("-fno-strict-aliasing")
        // Third-party sources: their warnings are upstream's to fix, and this
        // workspace builds with warnings denied.
        .warnings(false);

    for define in GC_DEFINES {
        build.define(define, None);
    }

    build.compile("gc");
}
