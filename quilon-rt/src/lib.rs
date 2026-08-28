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

//! Quilon runtime intrinsics — linked into every compiled Quilon program.
//!
//! These are `#[unsafe(no_mangle)] extern "C"` symbols so they resolve identically
//! from the in-process LLVM JIT (`quilon run`, via `add_global_mapping`) and from
//! ahead-of-time-linked native executables (`quilon compile` -> `llc` -> `gcc`,
//! linking `libquilon_rt.a`). The code generator declares matching external
//! prototypes and emits calls to these names; see `CodeGenerator::get_intrinsic`.
//!
//! This crate is built as both a `staticlib` (`libquilon_rt.a`, for AOT linking)
//! and an `rlib` (so the `quilon` binary embeds the same symbols for the JIT).
//!
//! The intrinsics are grouped by the surface they back: [`io`] (core.io — the one
//! genuinely lib-aligned module), [`text`] (the built-in `Text` type), [`process`]
//! (general process/runtime-lifecycle primitives: `__exit` and the entry-point
//! `argv`/`envp` conversions), [`test_registry`] (the counters behind `quilon test`), and
//! [`mem`] (general memory primitives: allocation,
//! GC, the shared `QlSlice` ABI type, bounds-check and range-endpoint failure). Each `#[no_mangle]`
//! intrinsic is re-exported at the crate root so callers reach it as
//! `quilon_rt::__name` regardless of which module defines it.
//!
//! Memory is managed by the Boehm conservative GC, compiled from the
//! `vendor/bdwgc` submodule by this crate's build script and linked statically;
//! the binding lives in [`mem`]. Because rustc bundles a static native library
//! into a staticlib, the collector travels inside `libquilon_rt.a` — an AOT-linked
//! Quilon binary needs no `libgc` on the machine that runs it.

pub mod collections;
pub mod deferred;
pub mod gc;
pub mod io;
pub mod mem;
pub mod net;
pub mod process;
pub mod reactor;
// Only the `QlSite` type is public (re-exported below); the formatter is the runtime's own.
mod report;
pub mod scheduler;
pub mod test_registry;
pub mod text;
pub mod time;

pub use collections::{
    __map_get, __map_has, __map_key_a, __map_key_b, __map_len, __map_new, __map_remove, __map_set,
    __map_val, __set_add, __set_diff, __set_has, __set_intersect, __set_item_a, __set_item_b,
    __set_len, __set_new, __set_remove, __set_union,
};
pub use deferred::{__force_result, __force_text, __read_launch, QlResult};
pub use io::{__color_enabled, __print_text_fd, __write_bytes};
pub use mem::{
    __alloc, __alloc_array, __gc_init, __index_fail, __range_endpoint, GcThread, MAX_EXACT_NUM,
    check_range_endpoint, register_thread,
};
pub use net::__tcp_request_launch;
pub use process::{__argv_to_text_array, __envp_to_map, __exit};
pub use report::{
    __assert_failed, __expect_failed, __match_fail, ASSERTION_EXIT_CODE, MAX_PATH_WIDTH, QlSite,
    shorten_path,
};
pub use scheduler::__run_fiber_main;
pub use test_registry::{
    __test_case_failing, __test_case_finish, __test_depth, __test_failed, __test_passed,
    __test_suite_enter, __test_suite_leave,
};
pub use text::{
    __bool_to_text, __num_to_text, __text_cmp, __text_contains, __text_index_of, __text_length,
    __text_repeat, __text_replace_all, __text_replace_n, __text_slice, __text_split,
    __text_to_lower, __text_to_upper, __text_trim_end, __text_trim_start,
};
pub use time::{__now, __sleep};

use mem::QlSlice;
use std::os::raw::{c_char, c_int, c_void};

/// Every runtime intrinsic, listed once.
///
/// Three things have to agree about this set, and each was maintained by hand: the
/// retention root, the JIT's name-to-address mapping, and the prototypes the code
/// generator declares. Adding an intrinsic to only two of them produced a call to a null
/// address at run time rather than a compile error — a segfault with no diagnostic. The
/// two that can be derived are generated from this list; the third is in another crate
/// (building a signature needs an LLVM context) and is held to it by a test and by
/// `get_intrinsic` refusing any name absent from `INTRINSICS`.
///
/// Each entry's signature is used only to erase — ABI-compatibly — to a common
/// fn-pointer type for storage. Nothing is ever called through either table.
type RtFn = unsafe extern "C" fn();

macro_rules! intrinsic_registry {
    ($($name:ident : $sig:ty),+ $(,)?) => {
        /// Name to address for every intrinsic, in declaration order. The JIT maps each
        /// declaration the code generator emitted onto the address here; an AOT link
        /// resolves the same names out of the archive instead.
        pub static INTRINSICS: &[(&str, RtFn)] = &[
            $((
                stringify!($name),
                unsafe { core::mem::transmute::<$sig, RtFn>($name as $sig) },
            )),+
        ];

        /// Force every runtime intrinsic to be RETAINED in the `staticlib` archive, even
        /// though nothing in this crate calls them (they are only ever called from the
        /// LLVM IR the code generator emits, which rustc never sees). Without an in-crate
        /// reference, the staticlib's link step could dead-strip an intrinsic, and this
        /// `#[used]` table is the reachability root that pins all of them into the archive; the
        /// AOT link then retains those members again at the executable link — under GNU ld with a
        /// narrow `-u <symbol>` per intrinsic, under ld64 by force-loading the archive.
        ///
        /// What checks that this still works is `tests/intrinsic_link_test.rs`: it builds
        /// a program reaching every intrinsic and links it under both linkers, so a
        /// dropped symbol is an undefined reference on every run. Prefer extending that
        /// gate over adding build settings — the last `undefined reference` scare here
        /// was diagnosed as dead-stripping and answered with a `codegen-units = 1`
        /// override, and turned out to be test binaries copying over the shared archive
        /// non-atomically while a sibling linked against it. The override only ever
        /// shrank the archive enough to narrow that window.
        ///
        /// Generated from the same list as `INTRINSICS`, so the two cannot drift — but it
        /// stays a plain `[RtFn; N]` array rather than borrowing the other table, because
        /// THIS shape is the one whose retention behaviour is established.
        #[allow(clippy::missing_transmute_annotations)]
        #[used]
        static QUILON_RT_INTRINSICS: [RtFn; [$(stringify!($name)),+].len()] = unsafe {
            [$(core::mem::transmute::<$sig, RtFn>($name as $sig)),+]
        };
    };
}

intrinsic_registry! {
    __gc_init: extern "C" fn(),
    __num_to_text: extern "C" fn(f64) -> QlSlice,
    __bool_to_text: extern "C" fn(i64) -> QlSlice,
    __exit: extern "C" fn(c_int) -> !,
    __index_fail: extern "C" fn(f64, i64, *const QlSite) -> !,
    __match_fail: extern "C" fn(*const QlSite) -> !,
    __range_endpoint: extern "C" fn(f64, *const QlSite) -> i64,
    __alloc: extern "C" fn(i64) -> *mut c_void,
    __alloc_array: extern "C" fn(i64, i64) -> *mut c_void,
    __text_length: extern "C" fn(*const u8, i64) -> i64,
    __text_cmp: extern "C" fn(*const u8, i64, *const u8, i64) -> i32,
    __write_bytes: extern "C" fn(i64, *const u8, i64) -> i64,
    __print_text_fd: extern "C" fn(i64, *const u8, i64),
    __color_enabled: extern "C" fn(i64) -> i64,
    __argv_to_text_array: extern "C" fn(i64, *const *const c_char) -> QlSlice,
    __envp_to_map: extern "C" fn(*const *const c_char) -> *mut c_void,
    __text_repeat: extern "C" fn(*const u8, i64, f64, *const QlSite) -> QlSlice,
    __text_trim_start: extern "C" fn(*const u8, i64) -> QlSlice,
    __text_trim_end: extern "C" fn(*const u8, i64) -> QlSlice,
    __text_to_upper: extern "C" fn(*const u8, i64) -> QlSlice,
    __text_to_lower: extern "C" fn(*const u8, i64) -> QlSlice,
    __text_contains: extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
    __text_index_of: extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
    __text_replace_all:
        extern "C" fn(*const u8, i64, *const u8, i64, *const u8, i64, *const QlSite) -> QlSlice,
    __text_replace_n: extern "C" fn(
        *const u8,
        i64,
        *const u8,
        i64,
        *const u8,
        i64,
        i64,
        *const QlSite,
    ) -> QlSlice,
    __text_slice: extern "C" fn(*const u8, i64, i64, i64) -> QlSlice,
    __text_split: extern "C" fn(*const u8, i64, *const u8, i64) -> QlSlice,
    __sleep: extern "C" fn(f64),
    __now: extern "C" fn() -> f64,
    __read_launch: extern "C" fn(*const QlSite) -> QlSlice,
    __tcp_request_launch: extern "C" fn(*mut QlResult, *const u8, i64, *const u8, i64),
    __force_text: extern "C" fn(*const c_void) -> QlSlice,
    __force_result: extern "C" fn(*mut QlResult, *const c_void),
    __run_fiber_main: extern "C" fn(
        extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int,
        c_int,
        *const *const c_char,
        *const *const c_char,
    ) -> c_int,
    __map_new: extern "C" fn() -> *mut c_void,
    __map_set: extern "C" fn(
        *const c_void,
        i64,
        i64,
        i64,
        *const c_void,
        *const c_void,
        *const c_void,
    ) -> *mut c_void,
    __map_remove:
        extern "C" fn(*const c_void, i64, i64, i64, *const c_void, *const c_void) -> *mut c_void,
    __map_get: extern "C" fn(
        *const c_void,
        i64,
        i64,
        i64,
        *const c_void,
        *const c_void,
        *mut i64,
    ) -> *const c_void,
    __map_has:
        extern "C" fn(*const c_void, i64, i64, i64, *const c_void, *const c_void) -> i64,
    __map_len: extern "C" fn(*const c_void) -> i64,
    __map_key_a: extern "C" fn(*const c_void, i64) -> i64,
    __map_key_b: extern "C" fn(*const c_void, i64) -> i64,
    __map_val: extern "C" fn(*const c_void, i64) -> *const c_void,
    __set_new: extern "C" fn() -> *mut c_void,
    __set_add:
        extern "C" fn(*const c_void, i64, i64, i64, *const c_void, *const c_void) -> *mut c_void,
    __set_remove:
        extern "C" fn(*const c_void, i64, i64, i64, *const c_void, *const c_void) -> *mut c_void,
    __set_has:
        extern "C" fn(*const c_void, i64, i64, i64, *const c_void, *const c_void) -> i64,
    __set_len: extern "C" fn(*const c_void) -> i64,
    __set_item_a: extern "C" fn(*const c_void, i64) -> i64,
    __set_item_b: extern "C" fn(*const c_void, i64) -> i64,
    __set_union: extern "C" fn(*const c_void, *const c_void) -> *mut c_void,
    __set_diff: extern "C" fn(*const c_void, *const c_void) -> *mut c_void,
    __set_intersect: extern "C" fn(*const c_void, *const c_void) -> *mut c_void,
    __test_suite_enter: extern "C" fn() -> f64,
    __test_suite_leave: extern "C" fn() -> f64,
    __test_depth: extern "C" fn() -> f64,
    __test_case_failing: extern "C" fn() -> f64,
    __test_case_finish: extern "C" fn() -> f64,
    __test_passed: extern "C" fn() -> f64,
    __test_failed: extern "C" fn() -> f64,
    __assert_failed: extern "C" fn(*const QlSite, *const u8, i64) -> !,
    __expect_failed: extern "C" fn(*const QlSite, *const u8, i64),
}

// Shared unit-test support. `GC_LOCK` is taken by GC-touching tests in more than one
// module; the `QlSlice` inspection helpers back the `text` tests. Both live here at the
// crate root so a single owner serves every module's test block.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::mem::QlSlice;
    use std::sync::Mutex;

    // libgc's `GC_init`/`GC_malloc` are not safe to invoke from several threads at
    // once; cargo runs tests in parallel, so every test that initializes/allocates
    // through the GC takes this lock first (mirrors `jit`'s JIT_LOCK).
    pub(crate) static GC_LOCK: Mutex<()> = Mutex::new(());

    /// View a `QlSlice` `Text` result as a `&str` (its GC-owned bytes). Takes the
    /// `QlSlice` by value (it is `Copy`) so the returned `&str` borrows the underlying
    /// GC buffer, not the (temporary) struct.
    pub(crate) unsafe fn slice_str<'a>(s: QlSlice) -> &'a str {
        let bytes = unsafe { std::slice::from_raw_parts(s.data as *const u8, s.len as usize) };
        std::str::from_utf8(bytes).unwrap()
    }

    pub(crate) fn text_of(s: &str) -> (*const u8, i64) {
        (s.as_ptr(), s.len() as i64)
    }

    /// Collect a `[]Text` `QlSlice` result into owned `String`s. Shared by the split tests.
    pub(crate) fn split_parts(s: &QlSlice) -> Vec<String> {
        let parts = unsafe { std::slice::from_raw_parts(s.data as *const QlSlice, s.len as usize) };
        parts
            .iter()
            .map(|p| unsafe { slice_str(*p) }.to_string())
            .collect()
    }
}

#[cfg(test)]
mod registry_tests {
    /// Every C-ABI symbol this crate exports must be in [`INTRINSICS`].
    ///
    /// The registry is what the JIT maps and what the retention table pins, so a symbol
    /// missing from it is one the JIT will call at a null address and the linker may drop
    /// — the exact failure the registry exists to prevent, arriving through the one door
    /// it does not watch. The code generator's parity test walks the registry outwards;
    /// nothing walked inwards from the exports until here.
    ///
    /// The crate's own source is the evidence: every intrinsic is a hand-written
    /// `#[unsafe(no_mangle)] pub extern "C" fn`, so scanning for that needs no build
    /// artifact and no `nm`, and it fails on the commit that adds the export rather than
    /// on the machine that later fails to link it.
    #[test]
    fn every_exported_symbol_is_registered() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut exported: Vec<(String, String)> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("reading the crate's source directory") {
            let path = entry.expect("reading a source entry").path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("reading a source file");
            let file = path.file_name().unwrap().to_string_lossy().to_string();
            for (index, line) in source.lines().enumerate() {
                // The exported form is a `pub extern "C" fn` under a no-mangle attribute;
                // the attribute may sit on the previous line, which is how they are written.
                let Some(rest) = line.trim().strip_prefix("pub extern \"C\" fn ") else {
                    continue;
                };
                let no_mangle = index
                    .checked_sub(1)
                    .and_then(|i| source.lines().nth(i))
                    .is_some_and(|prev| prev.contains("no_mangle"));
                if !no_mangle {
                    continue;
                }
                let name = rest
                    .split(['(', '<'])
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                exported.push((name, file.clone()));
            }
        }

        assert!(
            !exported.is_empty(),
            "found no exported intrinsics at all — this scan has stopped matching how they \
             are written, so it is no longer checking anything"
        );

        let unregistered: Vec<String> = exported
            .iter()
            .filter(|(name, _)| !super::INTRINSICS.iter().any(|(known, _)| known == name))
            .map(|(name, file)| format!("{name} ({file})"))
            .collect();
        assert!(
            unregistered.is_empty(),
            "these symbols are exported but missing from INTRINSICS: {unregistered:?} — the \
             JIT would call them at a null address and the linker may drop them; add them \
             to the intrinsic_registry! list"
        );
    }
}
