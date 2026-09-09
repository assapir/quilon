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
use std::sync::Mutex;
use unicode_segmentation::UnicodeSegmentation;

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
    fn GC_add_roots(low_address: *mut c_void, high_address_plus_1: *mut c_void);
    fn GC_remove_roots(low_address: *mut c_void, high_address_plus_1: *mut c_void);
}

/// Every range `__gc_add_root` has registered with Boehm and not yet removed, as
/// `(low_address, high_address_plus_1)` `usize` pairs (a `*mut c_void` is not `Send`), so
/// [`remove_registered_roots`] can remove each one individually rather than the whole set.
static ADDED_ROOTS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Boehm's description of a thread's stack extent, filled in by `GC_get_stack_base`.
#[repr(C)]
struct GcStackBase {
    mem_base: *mut c_void,
}

/// Initialize the garbage collector, and turn off `SIGPIPE`'s default disposition
/// (terminate the process) so a write to a closed pipe or socket reaches the caller as an
/// `EPIPE` error instead — the same failure `write_to_fd` already reports loudly under the
/// JIT, where the host process ignores `SIGPIPE` from the start. Emitted as the first call
/// in `main`.
#[unsafe(no_mangle)]
pub extern "C" fn __gc_init() {
    // Safe to call more than once; both calls are idempotent.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        GC_init()
    }
}

/// Register `bytes` bytes starting at `ptr` as an additional GC root: Boehm scans `.data`
/// but not memory the JIT maps at run time, so a computed global registers itself here
/// (harmless, redundant, under a native build).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __gc_add_root(ptr: *mut c_void, bytes: i64) {
    if bytes <= 0 {
        return;
    }
    // `GC_add_roots` takes an exclusive upper bound. SAFETY: `bytes` is this global's own
    // LLVM-computed size, so `ptr + bytes` stays within (one past) the allocation.
    let high = unsafe { ptr.add(bytes as usize) };
    unsafe { GC_add_roots(ptr, high) };
    ADDED_ROOTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((ptr as usize, high as usize));
}

/// Remove every root [`__gc_add_root`] has registered and not yet removed. A root points
/// into a JIT'd program's storage, which the execution engine frees once that run
/// returns, so a host running many programs in one process must call this between runs —
/// otherwise a later collection walks memory the engine has since freed or reused.
pub fn remove_registered_roots() {
    let mut roots = ADDED_ROOTS.lock().unwrap_or_else(|p| p.into_inner());
    for (low, high) in roots.drain(..) {
        unsafe { GC_remove_roots(low as *mut c_void, high as *mut c_void) };
    }
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
    let _ = write_to_fd(2, &line[..end]);
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
/// expression that asked for it, and terminate with exit status 5: the fail-loud contract of
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
/// `i64`, or a report at the range expression and exit status 5.
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
/// `data` points at a header (see `alloc_text_with_header`); for an array, `data` points
/// to `len` contiguous element-representation values and `len` is the element count.
/// `#[repr(C)]` so the field offsets (ptr at 0, i64 at 8) match what LLVM emits.
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

pub(crate) const TEXT_HEADER_BYTES: i64 = 16;

const TEXT_ASCII_ALIGNED: i64 = 1;
const TEXT_VALID_UTF8: i64 = 2;
const TEXT_LITERAL: i64 = 4;
const TEXT_NO_BIDI_CONTROLS: i64 = 8;

pub(crate) fn text_is_ascii(flags: i64) -> bool {
    flags & TEXT_ASCII_ALIGNED != 0
}

pub(crate) fn text_is_valid_utf8(flags: i64) -> bool {
    flags & TEXT_VALID_UTF8 != 0
}

pub(crate) fn text_no_bidi_controls(flags: i64) -> bool {
    flags & TEXT_NO_BIDI_CONTROLS != 0
}

/// Flags for a producer (`+`, `join`) whose output is always valid UTF-8 by construction —
/// a rearrangement of already-validated `Text` content — with `ascii`/`no_bidi` inherited
/// from its inputs' own headers.
pub(crate) fn inherited_flags(ascii: bool, no_bidi: bool) -> i64 {
    let mut flags = TEXT_VALID_UTF8;
    if ascii {
        flags |= TEXT_ASCII_ALIGNED;
    }
    if no_bidi {
        flags |= TEXT_NO_BIDI_CONTROLS;
    }
    flags
}

pub(crate) fn text_header_of(ptr: *const u8) -> (i64, i64) {
    if ptr.is_null() {
        return (
            0,
            TEXT_ASCII_ALIGNED | TEXT_VALID_UTF8 | TEXT_NO_BIDI_CONTROLS,
        );
    }
    // SAFETY: a non-null `data` always points at a header `alloc_text` (or a sibling) wrote.
    unsafe {
        let header = ptr as *const i64;
        (*header, *header.add(1))
    }
}

/// A lone `\r` disqualifies ASCII-aligned the same as a `\r\n` pair does, trading a
/// vanishingly rare false negative (a bare `\r`, alone, is still one grapheme's worth of
/// one byte) for `contains`'s memchr scan over the two-byte window check.
fn is_ascii_grapheme_aligned(bytes: &[u8]) -> bool {
    bytes.is_ascii() && !bytes.contains(&b'\r')
}

/// Every header bit free in the pass a producer already makes over `bytes`.
pub fn text_header(bytes: &[u8]) -> (i64, i64) {
    if is_ascii_grapheme_aligned(bytes) {
        let flags = TEXT_ASCII_ALIGNED | TEXT_VALID_UTF8 | TEXT_NO_BIDI_CONTROLS;
        return (bytes.len() as i64, flags);
    }
    let (text, valid_utf8) = match std::str::from_utf8(bytes) {
        Ok(s) => (std::borrow::Cow::Borrowed(s), true),
        Err(_) => (String::from_utf8_lossy(bytes), false),
    };
    let mut flags = if valid_utf8 { TEXT_VALID_UTF8 } else { 0 };

    // One pass: below U+0300 there are no combining marks, ZWJ, variation selectors,
    // regional indicators, or bidi controls, so every char is its own grapheme — except a
    // `\r` immediately followed by `\n` (GB3), which sits below that boundary too.
    let mut char_count: i64 = 0;
    let mut max_char = '\0';
    let mut has_cr = false;
    for ch in text.chars() {
        char_count += 1;
        max_char = max_char.max(ch);
        has_cr |= ch == '\r';
    }
    if max_char < '\u{0300}' && !has_cr {
        return (char_count, flags | TEXT_NO_BIDI_CONTROLS);
    }

    let count = text.graphemes(true).count() as i64;
    if !text.chars().any(crate::bidi::is_bidi_control) {
        flags |= TEXT_NO_BIDI_CONTROLS;
    }
    (count, flags)
}

/// [`text_header`], plus [`TEXT_LITERAL`] — the only place that bit is ever set.
pub fn literal_header(bytes: &[u8]) -> (i64, i64) {
    let (count, flags) = text_header(bytes);
    (count, flags | TEXT_LITERAL)
}

/// Every `Text` allocation carries [`TEXT_HEADER_BYTES`] (16) bytes before its content,
/// then a trailing NUL: an `i64` grapheme `count`, an `i64` `flags` (bit 0 ASCII AND
/// grapheme-aligned — a `\r\n` pair is all-ASCII but segments as one grapheme over two
/// bytes, so it needs its own condition; bit 1 valid UTF-8; bit 2 a compile-time literal;
/// bit 3 free of bidi control characters), the content, then one zero byte a C caller can
/// read as a NUL terminator with no copy (`__render_c_string`) — free, since `__alloc`
/// already zeroes. `data` points AT the header, not past it, so a debugger's `p *t.data`
/// shows every field alongside the bytes. Written once, here, for every producer.
pub(crate) fn alloc_text_with_header(bytes: &[u8], count: i64, flags: i64) -> QlSlice {
    let (slice, content) = alloc_text_buffer(bytes.len(), count, flags);
    if !content.is_null() {
        // SAFETY: `content` has room for exactly `bytes.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), content, bytes.len()) };
    }
    slice
}

/// The allocation `alloc_text_with_header` makes, split from the copy: `len` bytes of
/// header and trailing NUL already written, and a pointer to the `len` content bytes
/// still to fill — a producer building its content from more than one source (`+`, `join`)
/// writes each piece straight into place instead of assembling a Vec first. `content` is
/// null exactly when `len == 0` (the empty text, which needs no buffer).
pub(crate) fn alloc_text_buffer(len: usize, count: i64, flags: i64) -> (QlSlice, *mut u8) {
    if len == 0 {
        return (QlSlice::empty(), std::ptr::null_mut());
    }
    let buf = __alloc(TEXT_HEADER_BYTES + len as i64 + 1) as *mut u8;
    // SAFETY: `__alloc` returned at least that many writable bytes, zeroed (the trailing
    // NUL, at index `len`, needs no write of its own).
    let content = unsafe {
        let header = buf as *mut i64;
        header.write(count);
        header.add(1).write(flags);
        buf.add(TEXT_HEADER_BYTES as usize)
    };
    (
        QlSlice {
            data: buf as *const c_void,
            len: len as i64,
        },
        content,
    )
}

/// [`alloc_text_with_header`] when `count` is already known (`slice`, `at`) and `bytes` is
/// a byte-range of an already-decoded `&str`, so it is always valid UTF-8 for free too.
pub(crate) fn alloc_text_with_count(bytes: &[u8], count: i64) -> QlSlice {
    let flags = if is_ascii_grapheme_aligned(bytes) {
        TEXT_ASCII_ALIGNED | TEXT_VALID_UTF8 | TEXT_NO_BIDI_CONTROLS
    } else {
        TEXT_VALID_UTF8
    };
    alloc_text_with_header(bytes, count, flags)
}

pub(crate) fn alloc_text(bytes: &[u8]) -> QlSlice {
    let (count, flags) = text_header(bytes);
    alloc_text_with_header(bytes, count, flags)
}

/// A NUL-terminated view of the `len` bytes at `data`, no copy: every non-empty `Text`
/// (an allocation or a literal constant) already carries a trailing NUL. Backs every
/// `--debug` build's `__qn_render$...` thunks (see `CodeGenerator::emit_render_thunk`): a
/// debugger evaluates a C-ABI call and reads back a `const char*`, not the `{ ptr, i64 }`
/// ABI a Quilon caller uses. Not reachable from a `.qn` program.
///
/// # Safety contract (upheld by the compiler)
/// `data` is a `Text`'s own `data` field (null, or a header followed by `len` bytes and a
/// trailing NUL).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __render_c_string(data: *const u8, len: i64) -> *const u8 {
    if data.is_null() || len <= 0 {
        return c"".as_ptr().cast();
    }
    // SAFETY: upheld by the caller (see the contract above).
    unsafe { data.add(TEXT_HEADER_BYTES as usize) }
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
    fn gc_add_root_accepts_a_fresh_allocation() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let p = __alloc(16);
        __gc_add_root(p, 16);
        // `ADDED_ROOTS` is a process-wide list every test in this file shares — leaving
        // this root registered would leak into whichever GC-touching test runs next.
        remove_registered_roots();
    }

    #[test]
    fn gc_add_root_of_non_positive_size_is_a_no_op() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        __gc_add_root(std::ptr::null_mut(), 0);
        __gc_add_root(std::ptr::null_mut(), -1);
    }

    /// A range `__gc_add_root` registers is exactly what `remove_registered_roots` hands
    /// back to `GC_remove_roots`, and a fresh range registers again cleanly afterward —
    /// what the JIT harness needs between one program's run and the next.
    #[test]
    fn gc_add_root_then_remove_then_add_again_round_trips() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();

        let first = __alloc(16);
        __gc_add_root(first, 16);
        assert_eq!(
            ADDED_ROOTS.lock().unwrap_or_else(|p| p.into_inner()).len(),
            1
        );

        remove_registered_roots();
        assert!(
            ADDED_ROOTS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );

        let second = __alloc(16);
        __gc_add_root(second, 16);
        assert_eq!(
            ADDED_ROOTS.lock().unwrap_or_else(|p| p.into_inner()).len(),
            1
        );

        remove_registered_roots();
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
    fn render_c_string_is_a_view_of_the_texts_own_trailing_nul_no_copy() {
        let (ptr, len) = crate::test_support::text_of("hi");
        let out = __render_c_string(ptr, len);
        assert_eq!(out, unsafe { ptr.add(TEXT_HEADER_BYTES as usize) });
        // SAFETY: `text_of` appended the same trailing NUL a real allocation carries.
        let viewed = unsafe { std::slice::from_raw_parts(out, len as usize + 1) };
        assert_eq!(viewed, b"hi\0");
    }

    #[test]
    fn render_c_string_of_zero_length_is_a_static_empty_string() {
        let out = __render_c_string(std::ptr::null(), 0);
        assert!(!out.is_null());
        // SAFETY: `out` is a static, NUL-terminated C string.
        assert_eq!(unsafe { *out }, 0);
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

    #[test]
    fn header_bits_for_ascii_text() {
        let (_, flags) = text_header(b"hello");
        assert!(text_is_ascii(flags));
        assert!(text_is_valid_utf8(flags));
        assert!(text_no_bidi_controls(flags));
    }

    #[test]
    fn header_bits_for_non_ascii_valid_utf8() {
        let (_, flags) = text_header("héllo".as_bytes());
        assert!(!text_is_ascii(flags));
        assert!(text_is_valid_utf8(flags));
        assert!(text_no_bidi_controls(flags));
    }

    #[test]
    fn header_bits_for_invalid_utf8() {
        let (_, flags) = text_header(b"a\xFFb");
        assert!(!text_is_ascii(flags));
        assert!(!text_is_valid_utf8(flags));
        assert!(text_no_bidi_controls(flags));
    }

    #[test]
    fn header_bits_for_a_bidi_control() {
        let (_, flags) = text_header("a\u{202E}b".as_bytes());
        assert!(!text_is_ascii(flags));
        assert!(text_is_valid_utf8(flags));
        assert!(!text_no_bidi_controls(flags));
    }

    /// The direct grapheme walk `text_header` falls back to when its below-U+0300 fast
    /// path does not apply — the reference every fast-path count is checked against.
    fn walk_count(s: &str) -> i64 {
        s.graphemes(true).count() as i64
    }

    #[test]
    fn below_u0300_fast_path_matches_the_grapheme_walk() {
        for s in ["äb", "héllo wörld", "日本"] {
            let (count, _) = text_header(s.as_bytes());
            assert_eq!(count, walk_count(s), "{s:?}");
        }
    }

    #[test]
    fn a_combining_mark_still_counts_one_grapheme() {
        // "e" + a combining acute (U+0301, at the fast path's own boundary) is one
        // grapheme, not two — the fast path must not apply here.
        let (count, flags) = text_header("e\u{0301}".as_bytes());
        assert_eq!(count, 1);
        assert!(text_no_bidi_controls(flags));
    }

    #[test]
    fn a_flag_emoji_still_counts_one_grapheme() {
        let (count, _) = text_header("🇮🇱".as_bytes());
        assert_eq!(count, 1);
    }

    #[test]
    fn literal_header_adds_the_literal_bit_on_top_of_text_header() {
        let (count, flags) = literal_header(b"hi");
        let (plain_count, plain_flags) = text_header(b"hi");
        assert_eq!(count, plain_count);
        assert_eq!(flags, plain_flags | TEXT_LITERAL);
    }
}
