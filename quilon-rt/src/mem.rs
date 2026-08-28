// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Internal runtime primitives with no `core.*` language home: allocation and the
//! Boehm-GC binding (`__alloc`, `__gc_init`), the shared `QlSlice` `{ ptr, len }`
//! ABI type and its `alloc_text` helper, the `format_num` render helper, and the
//! fail-loud primitives behind checked `arr[i]` (`__index_fail`) and a range's
//! endpoints (`__range_endpoint`) — neither operation has a `core.*` module, so both
//! live in this internal tier. This tier is where the future fiber scheduler and
//! reactor will also live.

use crate::report::{QlSite, RUNTIME_EXIT_CODE, fail_at};
use std::os::raw::c_void;

// The Boehm GC, compiled from the `vendor/bdwgc` submodule by this crate's build
// script and linked statically, so a compiled Quilon program carries its own
// collector and needs no `libgc` installed where it runs.
#[link(name = "gc", kind = "static")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
    fn GC_allow_register_threads();
    fn GC_register_my_thread(sb: *const GcStackBase) -> i32;
    fn GC_unregister_my_thread() -> i32;
    fn GC_get_stack_base(sb: *mut GcStackBase) -> i32;
}

/// Boehm's description of a thread's stack extent, filled in by `GC_get_stack_base`.
#[repr(C)]
struct GcStackBase {
    mem_base: *mut c_void,
}

/// Initialize the garbage collector. Emitted as the first call in `main`.
#[unsafe(no_mangle)]
pub extern "C" fn __gc_init() {
    // Safe to call more than once; GC_init is idempotent.
    unsafe { GC_init() }
}

/// Prepare the collector for threads other than the one that initialized it, and
/// register the calling thread with it until the returned guard is dropped.
///
/// The collector stops the world by signalling the threads it knows, and it only knows
/// the thread that initialized it plus any it was told about. A thread it has not been
/// told about is not merely unscanned: when a collection happens the process aborts,
/// with a message that varies by timing — `Collecting from unknown thread`,
/// `pthread_kill failed at suspend`, `Signals delivery fails constantly`. A compiled
/// Quilon program never meets this, because it has one thread. A *host* that runs
/// Quilon code on more than one thread does, which is why the JIT calls this.
///
/// Initialization runs once however many threads arrive at it together: two threads
/// initializing at the same time abort with `Exclusion ranges overlap`.
pub fn register_thread() -> GcThread {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        GC_init();
        GC_allow_register_threads();
    });

    let registered = unsafe {
        let mut base = GcStackBase {
            mem_base: std::ptr::null_mut(),
        };
        GC_get_stack_base(&mut base);
        // 0 says we registered it, 1 says it was already known — see `GcThread` for why
        // both count as ours to remove.
        matches!(GC_register_my_thread(&base), 0 | 1)
    };
    GcThread { registered }
}

/// Unregisters the thread when it is dropped.
///
/// Taking the thread back out matters as much as putting it in, because the collector's
/// knowledge outlives the thread: an entry left behind is a corpse every later collection
/// tries to stop. That includes the thread that initialized the collector, which is
/// already known without registering — so it is unregistered here too.
pub struct GcThread {
    registered: bool,
}

impl Drop for GcThread {
    fn drop(&mut self) {
        if self.registered {
            unsafe { GC_unregister_my_thread() };
        }
    }
}

/// Allocate `size` bytes of GC-managed, zeroed-on-demand memory.
///
/// Returns a pointer the collector tracks; callers never free it. A non-positive
/// size yields a 1-byte allocation so the result is always a valid, unique-ish
/// pointer.
#[unsafe(no_mangle)]
pub extern "C" fn __alloc(size: i64) -> *mut c_void {
    let n = if size <= 0 { 1 } else { size as usize };
    unsafe { GC_malloc(n) }
}

/// Report an invalid array index — out of bounds, negative, or NaN — at the indexing
/// expression that asked for it, and terminate with exit status 1: the fail-loud contract of
/// checked `arr[i]` indexing.
///
/// `index` is the ORIGINAL f64 the program computed (pre-truncation), so the message shows
/// what the user actually asked for; `size` is the array's element count; `site` is the
/// `arr[i]` expression's own location, which the report frames the same way a failing
/// assertion does. Codegen calls this from the invalid branch of every `arr[i]` bounds check.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
#[unsafe(no_mangle)]
pub extern "C" fn __index_fail(index: f64, size: i64, site: *const QlSite) -> ! {
    fail_at(
        site,
        &format!(
            "index {} out of bounds for an array of size {}",
            format_num(index),
            size
        ),
        RUNTIME_EXIT_CODE,
    )
}

/// One past the largest `f64` an `i64` can hold (2^63). `i64::MAX` itself is not
/// representable as an `f64`, so the bound is stated as the power of two both types
/// round to, and tested with a half-open comparison.
const I64_BOUND: f64 = 9_223_372_036_854_775_808.0;

/// A range endpoint as the whole number it must be, or the message saying why it is not.
///
/// `lo <- hi` counts from one endpoint to the other, which only means anything for whole
/// numbers that fit the counter: a fractional end has no next element, NaN has no order,
/// and a magnitude past `i64` has no representation to count in. Each of those is an
/// ERROR, never a truncation — `1.5 <- 3.9` is not `[1, 2, 3]`.
///
/// Shared with the compiler, which applies it to a literal endpoint at compile time so the
/// static and the runtime rejection read identically.
pub fn check_range_endpoint(value: f64) -> Result<i64, String> {
    if !value.is_finite() || value.fract() != 0.0 {
        return Err(format!(
            "a range endpoint must be a whole number (got {})",
            format_num(value)
        ));
    }
    if !(-I64_BOUND..I64_BOUND).contains(&value) {
        return Err(format!(
            "a range endpoint must be a whole number that fits 64 bits (got {})",
            format_num(value)
        ));
    }
    Ok(value as i64)
}

/// The checked `f64` -> `i64` conversion of one endpoint of `lo <- hi`: the endpoint as an
/// `i64`, or a report at the range expression and exit status 1.
///
/// Codegen calls this instead of emitting `fptosi`, which is where the unchecked version
/// went wrong: converting a NaN or an out-of-range `f64` yields poison, and a constant one
/// folds to poison before the range's allocation is even sized.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
#[unsafe(no_mangle)]
pub extern "C" fn __range_endpoint(value: f64, site: *const QlSite) -> i64 {
    match check_range_endpoint(value) {
        Ok(endpoint) => endpoint,
        Err(message) => fail_at(site, &message, RUNTIME_EXIT_CODE),
    }
}

/// A Quilon `Text` value (also the representation of an array): `{ ptr data, i64 len }`,
/// matching the code generator's `ptr_len_struct_type` (`{ i8*, i64 }`). For a `Text`,
/// `data` points to `len` UTF-8 bytes; for an array, `data` points to `len` contiguous
/// element-representation values and `len` is the element count. `#[repr(C)]` so the
/// field offsets (ptr at 0, i64 at 8) match what LLVM emits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QlSlice {
    pub(crate) data: *const c_void,
    pub(crate) len: i64,
}

impl QlSlice {
    /// The empty slice (`{ null, 0 }`) — a zero-length `Text`/array. Returned when there
    /// is nothing to build (null/empty `argv`/`envp`).
    pub(crate) fn empty() -> QlSlice {
        QlSlice {
            data: std::ptr::null(),
            len: 0,
        }
    }

    /// This slice's bytes decoded as `Text` — empty when it carries none (a null pointer or
    /// a non-positive length). Reads it exactly as the `Text` intrinsics do.
    ///
    /// # Safety contract (upheld by the compiler)
    /// A non-null `data` points to at least `len` readable bytes.
    pub(crate) fn as_text(&self) -> std::borrow::Cow<'_, str> {
        crate::text::text_str(self.data as *const u8, self.len)
    }

    /// Whether this slice carries no bytes.
    pub(crate) fn is_empty(&self) -> bool {
        self.data.is_null() || self.len <= 0
    }
}

/// GC-allocate a `Text` whose bytes are a copy of `bytes`. The copy is owned by the GC, so
/// it outlives the C `argv`/`envp` buffers, which the program may not keep. A `Text` is
/// exactly its `{ ptr, len }` bytes — nothing reads past `len`.
pub(crate) fn alloc_text(bytes: &[u8]) -> QlSlice {
    let len = bytes.len();
    let buf = __alloc(len as i64) as *mut u8;
    if !buf.is_null() && len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len) };
    }
    QlSlice {
        data: buf as *const c_void,
        len: len as i64,
    }
}

/// Render an `f64` the way Quilon shows a `Num`: whole values without a fractional part
/// (`5`, not `5.0`), everything else in shortest round-trip form. Shared by `__num_to_text`
/// and the `__index_fail` diagnostic.
pub(crate) fn format_num(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::GC_LOCK;

    #[test]
    fn format_num_drops_trailing_zeros_for_whole_values() {
        assert_eq!(format_num(3.0), "3");
        assert_eq!(format_num(120.0), "120");
        assert_eq!(format_num(3.5), "3.5");
    }

    #[test]
    fn a_whole_range_endpoint_converts() {
        assert_eq!(check_range_endpoint(0.0), Ok(0));
        assert_eq!(check_range_endpoint(-4.0), Ok(-4));
        assert_eq!(check_range_endpoint(1e15), Ok(1_000_000_000_000_000));
    }

    #[test]
    fn a_range_endpoint_that_is_not_whole_is_refused() {
        for (value, shown) in [(1.5, "1.5"), (f64::NAN, "NaN"), (f64::INFINITY, "inf")] {
            let message = check_range_endpoint(value).expect_err("must be refused");
            assert_eq!(
                message,
                format!("a range endpoint must be a whole number (got {shown})")
            );
        }
    }

    #[test]
    fn a_range_endpoint_wider_than_an_i64_is_refused() {
        // 2^63 is the first whole `f64` with no `i64` to convert to; -2^63 is `i64::MIN`
        // and stays legal, so the bound is half-open rather than symmetric.
        assert_eq!(check_range_endpoint(-I64_BOUND), Ok(i64::MIN));
        let message = check_range_endpoint(I64_BOUND).expect_err("must be refused");
        assert!(
            message.starts_with("a range endpoint must be a whole number that fits 64 bits"),
            "{message}"
        );
    }

    #[test]
    fn alloc_returns_usable_memory() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let p = __alloc(16) as *mut u8;
        assert!(!p.is_null());
        unsafe {
            std::ptr::write_bytes(p, 0xAB, 16);
            assert_eq!(*p, 0xAB);
        }
    }
}
