// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Internal runtime primitives with no `core.*` language home: allocation and the
//! Boehm-GC binding (`__alloc`, `__alloc_array`, `__gc_init`), the shared `QlSlice`
//! `{ ptr, len }` ABI type and its `alloc_text` helper, the `format_num` render helper,
//! and the fail-loud primitives behind checked `arr[i]` (`__index_fail`) and a range's
//! endpoints (`__range_endpoint`) — neither operation has a `core.*` module,
//! so both live in this internal tier. This tier is where the future fiber scheduler and
//! reactor will also live.

use crate::io::write_to_fd;
use crate::process::__exit;
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
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

/// Report `message` with no call site to frame it — the allocation checks run below the
/// expression that asked, so there is no span to point at — and terminate.
///
/// Kept out of line so the formatting it does stays off the allocation path: what the
/// allocators carry is a test and a branch to here.
#[cold]
#[inline(never)]
fn alloc_fail(message: &str) -> ! {
    fail_at(
        std::ptr::null(),
        codes::ALLOCATION_FAILED,
        message,
        RUNTIME_EXIT_CODE,
    )
}

/// Allocate `size` bytes of GC-managed, zeroed-on-demand memory.
///
/// Returns a pointer the collector tracks; callers never free it. A zero size yields a
/// 1-byte allocation so the result is always a valid, unique-ish pointer.
///
/// NEVER returns null, and never quietly shrinks a request. A collector that cannot satisfy
/// the size aborts here, with the size it could not find; so does a NEGATIVE size, which is
/// a size computation that went wrong upstream — clamped to one byte, it becomes a block the
/// caller then fills as if it were the size it asked for. Handing either back is a
/// `Text`/array whose `data` is null or too small while its `len` says otherwise, and the
/// first read turns that into undefined behavior far from the allocation that failed.
#[unsafe(no_mangle)]
pub extern "C" fn __alloc(size: i64) -> *mut c_void {
    if size < 0 {
        alloc_fail(&format!("invalid allocation: {size} bytes"));
    }
    let n = if size == 0 { 1 } else { size as usize };
    // SAFETY: `GC_malloc` is the collector's allocation entry point; `n` is positive.
    let block = unsafe { GC_malloc(n) };
    if block.is_null() {
        out_of_memory(n);
    }
    block
}

/// Report an allocation the collector could not satisfy and terminate — WITHOUT allocating.
///
/// This is the one report that cannot afford a `String`: the collector just failed to find
/// memory, and where the process is genuinely out of it the global allocator is next, which
/// aborts on failure rather than returning. So the line is built in a stack buffer and
/// written straight to stderr, in the same shape a site-less report prints.
#[cold]
#[inline(never)]
fn out_of_memory(size: usize) -> ! {
    const PREFIX: &[u8] = b"out of memory: cannot allocate ";
    const SUFFIX: &[u8] = b" bytes\n";

    // The size in decimal, filled from the end (a `usize` is at most 20 digits).
    let mut digits = [0u8; 20];
    let mut first = digits.len();
    let mut left = size;
    loop {
        first -= 1;
        digits[first] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }

    let mut line = [0u8; PREFIX.len() + 20 + SUFFIX.len()];
    let mut end = 0;
    for part in [PREFIX, &digits[first..], SUFFIX] {
        line[end..end + part.len()].copy_from_slice(part);
        end += part.len();
    }
    write_to_fd(2, &line[..end]);
    __exit(RUNTIME_EXIT_CODE)
}

/// Allocate the backing store for `count` values of `elem_size` bytes each — the array
/// allocation, with the size computed HERE so the multiplication is checked.
///
/// Left to wrap in the caller, a `count * elem_size` too large for an `i64` lands on a
/// non-positive size, and the fill that follows writes `count` elements into the one byte
/// that comes back. A negative operand is reported as what it is, before the multiply turns
/// it into an overflow.
#[unsafe(no_mangle)]
pub extern "C" fn __alloc_array(count: i64, elem_size: i64) -> *mut c_void {
    if count < 0 || elem_size < 0 {
        alloc_fail(&format!(
            "invalid allocation: {count} elements of {elem_size} bytes each"
        ));
    }
    match count.checked_mul(elem_size) {
        Some(bytes) => __alloc(bytes),
        None => alloc_fail(&format!(
            "allocation too large: {count} elements of {elem_size} bytes each exceeds the \
             largest representable size"
        )),
    }
}

/// GC-allocate room for `count` values of `T`, through the checked array allocation.
pub(crate) fn alloc_slots<T>(count: usize) -> *mut T {
    __alloc_array(count as i64, std::mem::size_of::<T>() as i64) as *mut T
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
        codes::INDEX_OUT_OF_BOUNDS,
        &format!(
            "index {} out of bounds for an array of size {}",
            format_num(index),
            size
        ),
        RUNTIME_EXIT_CODE,
    )
}

/// 2^53 — the largest whole number a `Num` (an `f64`) represents exactly. Past it,
/// consecutive integers collide: 2^53 + 1 is not representable. Inclusive at both ends,
/// since 2^53 itself is exact.
pub const MAX_EXACT_NUM: f64 = (1u64 << 53) as f64;

/// A range endpoint as the whole number it must be, or the message saying why it is not.
///
/// `lo <- hi` counts from one endpoint to the other, so an end must be a whole number a
/// `Num` holds exactly: at most [`MAX_EXACT_NUM`] in magnitude. A fractional end, NaN, and
/// an infinity are all refused.
pub fn check_range_endpoint(value: f64) -> Result<i64, String> {
    if value.fract() != 0.0 && !value.is_infinite() {
        return Err(format!(
            "a range endpoint must be a whole number (got {})",
            format_num(value)
        ));
    }
    if value.abs() > MAX_EXACT_NUM {
        return Err(format!(
            "a range endpoint must be a whole number a Num holds exactly, at most {} in \
             magnitude (got {})",
            format_num(MAX_EXACT_NUM),
            format_num(value)
        ));
    }
    Ok(value as i64)
}

/// [`check_range_endpoint`] for an end the compiler could not evaluate: the endpoint as an
/// `i64`, or a report at the range expression and exit status 1.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
#[unsafe(no_mangle)]
pub extern "C" fn __range_endpoint(value: f64, site: *const QlSite) -> i64 {
    match check_range_endpoint(value) {
        Ok(endpoint) => endpoint,
        Err(message) => fail_at(
            site,
            codes::RANGE_ENDPOINT_NOT_WHOLE,
            &message,
            RUNTIME_EXIT_CODE,
        ),
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
    if len > 0 {
        // SAFETY: `__alloc` returned at least `len` writable bytes (it aborts rather
        // than returning null), and a fresh allocation cannot overlap `bytes`.
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
        for (value, shown) in [(1.5, "1.5"), (-0.25, "-0.25"), (f64::NAN, "NaN")] {
            let message = check_range_endpoint(value).expect_err("must be refused");
            assert_eq!(
                message,
                format!("a range endpoint must be a whole number (got {shown})")
            );
        }
    }

    #[test]
    fn a_range_endpoint_a_num_cannot_hold_exactly_is_refused() {
        // The limit itself is exact, so both signs of it are legal — the bound is inclusive.
        assert_eq!(
            check_range_endpoint(MAX_EXACT_NUM),
            Ok(9_007_199_254_740_992)
        );
        assert_eq!(
            check_range_endpoint(-MAX_EXACT_NUM),
            Ok(-9_007_199_254_740_992)
        );
        // An infinity is whole, so it fails on magnitude, not on being fractional.
        for value in [
            MAX_EXACT_NUM * 2.0,
            -MAX_EXACT_NUM * 2.0,
            1e19,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let message = check_range_endpoint(value).expect_err("must be refused");
            assert!(
                message.starts_with("a range endpoint must be a whole number a Num holds exactly"),
                "{message}"
            );
        }
    }

    /// The largest legal endpoints are ~2^53 apart, so an inclusive count of them is ~2^54 —
    /// far inside an `i64`. Nothing downstream needs a widened span.
    #[test]
    fn the_widest_legal_span_fits_a_count() {
        let widest = 2.0 * MAX_EXACT_NUM + 1.0;
        assert!(widest < i64::MAX as f64, "{widest}");
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

    #[test]
    fn an_array_allocation_is_the_product_of_its_operands() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // Every byte the count and element size ask for is writable — the check the
        // multiplication exists for, since a wrapped product yields a single byte.
        let p = __alloc_array(64, 8) as *mut u8;
        assert!(!p.is_null());
        unsafe {
            std::ptr::write_bytes(p, 0xCD, 64 * 8);
            assert_eq!(*p.add(64 * 8 - 1), 0xCD);
        }
    }

    #[test]
    fn an_empty_array_still_allocates() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // A zero-element array is ordinary, not a failure: it gets the same 1-byte
        // placeholder `__alloc(0)` gives, so its data pointer is still valid.
        assert!(!__alloc_array(0, 8).is_null());
    }

    #[test]
    fn slots_size_themselves_from_the_type() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let slots = alloc_slots::<QlSlice>(3);
        assert!(!slots.is_null());
        unsafe {
            for i in 0..3 {
                std::ptr::write(slots.add(i), QlSlice::empty());
            }
            assert_eq!((*slots.add(2)).len, 0);
        }
    }
}
