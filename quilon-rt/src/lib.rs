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
//! Memory is managed by the Boehm conservative GC (libgc). `__alloc` forwards to
//! `GC_malloc` and `__gc_init` to `GC_init`; both are referenced here so the
//! linker keeps libgc loaded. libgc must be installed (`libgc-dev` / `gc`).
//! When linking an AOT binary with gcc, pass `-lgc` explicitly (the `#[link]`
//! directive below only drives rustc's own links, not a downstream gcc invocation).

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use unicode_segmentation::UnicodeSegmentation;

// Link the Boehm GC and tie it to these symbol references so the linker keeps
// libgc for every target (binary, tests, JIT harness) regardless of `--as-needed`
// ordering. libgc must be installed (`libgc-dev` / `gc`); CI installs it.
#[link(name = "gc")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
}

/// Initialize the garbage collector. Emitted as the first call in `main`.
#[unsafe(no_mangle)]
pub extern "C" fn __gc_init() {
    // Safe to call more than once; GC_init is idempotent.
    unsafe { GC_init() }
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

/// Count the user-perceived characters (Unicode extended grapheme clusters) in a
/// UTF-8 byte buffer. Backs `Text.length`. Invalid UTF-8 is decoded lossily.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` points to at least `len` readable bytes (or is null with `len <= 0`).
// Exported C-ABI symbol called from generated code; a safe Rust signature is
// intentional (the contract is upheld by the compiler emitting the call).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_length(ptr: *const u8, len: i64) -> i64 {
    if ptr.is_null() || len <= 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    match std::str::from_utf8(bytes) {
        Ok(s) => s.graphemes(true).count() as i64,
        Err(_) => String::from_utf8_lossy(bytes).graphemes(true).count() as i64,
    }
}

/// Lexicographically compare two UTF-8 byte strings, returning -1, 0, or 1 (like
/// `memcmp`/Rust's `Ord` on byte slices: a common prefix orders by length). Backs the
/// `Text` comparison operators (`==`/`!=`/`<`/`<=`/`>`/`>=`).
///
/// # Safety contract (upheld by the compiler)
/// `a`/`b` are null or point to at least `alen`/`blen` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_cmp(a: *const u8, alen: i64, b: *const u8, blen: i64) -> i32 {
    let lhs = byte_slice(a, alen);
    let rhs = byte_slice(b, blen);
    match lhs.cmp(rhs) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// View `len` bytes at `ptr` as a slice (empty for null/non-positive `len`).
fn byte_slice<'a>(ptr: *const u8, len: i64) -> &'a [u8] {
    if ptr.is_null() || len <= 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    }
}

/// Write `len` bytes from `ptr` to file descriptor `fd`, returning the number of
/// bytes written (0 on null/empty/error). Backs the `write(content, fd)` builtin.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to at least `len` readable bytes; `fd` is a valid
/// descriptor (e.g. `stdout`=1, `stderr`=2). The borrowed fd is never closed.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __write_bytes(fd: i64, ptr: *const u8, len: i64) -> i64 {
    if ptr.is_null() || len <= 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    write_to_fd(fd, bytes)
}

/// Format and write a number to `fd` followed by a newline (backs `print`/`eprint`
/// of a `Num`). Whole values print without a fractional part (`3`, not `3.0`).
#[unsafe(no_mangle)]
pub extern "C" fn __print_num_fd(fd: i64, x: f64) {
    write_to_fd(fd, format!("{}\n", format_num(x)).as_bytes());
}

/// Write `true`/`false` to `fd` followed by a newline (backs `print`/`eprint` of
/// a `Bool`). `b` is the bool zero-extended to an integer (0 = false).
#[unsafe(no_mangle)]
pub extern "C" fn __print_bool_fd(fd: i64, b: i64) {
    write_to_fd(fd, if b != 0 { b"true\n" } else { b"false\n" });
}

/// Write a NUL-terminated C string to `fd` followed by a newline (backs
/// `print`/`eprint` of a `Text`).
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to a NUL-terminated byte string.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __print_text_fd(fd: i64, ptr: *const c_char) {
    let mut s = cstr_to_str(ptr).unwrap_or_default().into_owned();
    s.push('\n');
    write_to_fd(fd, s.as_bytes());
}

/// Write all `bytes` to descriptor `fd` without closing it. Returns bytes written.
///
/// Uses libc `write(2)` directly rather than `std::fs::File`. AOT-linked native
/// binaries enter through the LLVM-generated C `main`, so the Rust std runtime is
/// never initialized and std's higher-level I/O does not work there — a raw
/// syscall does, and resolves identically under the JIT.
fn write_to_fd(fd: i64, bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    // SAFETY: `fd` is a live descriptor owned by the running program; we only
    // write to it (never close it). `buf`/`count` describe a valid byte slice.
    unsafe extern "C" {
        fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
    }
    let mut total = 0usize;
    while total < bytes.len() {
        let n = unsafe {
            write(
                fd as i32,
                bytes[total..].as_ptr() as *const c_void,
                bytes.len() - total,
            )
        };
        if n <= 0 {
            break;
        }
        total += n as usize;
    }
    total as i64
}

fn cstr_to_str<'a>(ptr: *const c_char) -> Option<std::borrow::Cow<'a, str>> {
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy())
}

fn format_num(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    }
}

/// Force every runtime intrinsic to be RETAINED in the `staticlib` archive, even
/// though nothing in this crate calls them (they are only ever called from the
/// LLVM IR the code generator emits, which rustc never sees). Without an in-crate
/// reference, the staticlib's link step can dead-strip an intrinsic — observed in
/// CI as `undefined reference to __text_cmp` during AOT linking while the JIT (which
/// maps symbols by address) was unaffected. The `#[used]` table is a reachability
/// root that pins all of them deterministically, independent of codegen-unit layout
/// or linker GC. (The AOT link also wraps the archive in `--whole-archive`; this
/// guarantees the symbols are present to be pulled in the first place.)
// Function pointers transmuted to a common fn-pointer type — `Sync`,
// const-constructible, and each entry pins its intrinsic. Kept as a `#[used]`
// reachability root so the staticlib link never dead-strips an intrinsic that is
// only ever called from generated LLVM IR (never from Rust). All entries are plain
// `extern "C"` fn items; the transmute only erases their (ABI-compatible) parameter
// lists for storage — the pointers are never called through this array.
type RtFn = unsafe extern "C" fn();
// Each `transmute` only erases an (ABI-irrelevant) parameter list to a common
// fn-pointer type for storage; the entries are never called through this array.
#[allow(clippy::missing_transmute_annotations)]
#[used]
static QUILON_RT_INTRINSICS: [RtFn; 8] = unsafe {
    [
        core::mem::transmute(__gc_init as extern "C" fn()),
        core::mem::transmute(__alloc as extern "C" fn(i64) -> *mut c_void),
        core::mem::transmute(__text_length as extern "C" fn(*const u8, i64) -> i64),
        core::mem::transmute(__text_cmp as extern "C" fn(*const u8, i64, *const u8, i64) -> i32),
        core::mem::transmute(__write_bytes as extern "C" fn(i64, *const u8, i64) -> i64),
        core::mem::transmute(__print_num_fd as extern "C" fn(i64, f64)),
        core::mem::transmute(__print_bool_fd as extern "C" fn(i64, i64)),
        core::mem::transmute(__print_text_fd as extern "C" fn(i64, *const c_char)),
    ]
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_count_handles_ascii_and_multibyte() {
        let ascii = b"hello";
        assert_eq!(__text_length(ascii.as_ptr(), ascii.len() as i64), 5);

        // "héllo" — the é is 2 bytes but 1 grapheme.
        let multibyte = "héllo".as_bytes();
        assert_eq!(multibyte.len(), 6);
        assert_eq!(__text_length(multibyte.as_ptr(), multibyte.len() as i64), 5);
    }

    #[test]
    fn grapheme_count_handles_emoji_clusters() {
        // Family emoji (ZWJ sequence) is many bytes / codepoints but one grapheme.
        let family = "👨‍👩‍👧".as_bytes();
        assert!(family.len() > 4);
        assert_eq!(__text_length(family.as_ptr(), family.len() as i64), 1);
    }

    #[test]
    fn text_length_null_and_empty_are_zero() {
        assert_eq!(__text_length(std::ptr::null(), 0), 0);
        assert_eq!(__text_length(b"x".as_ptr(), 0), 0);
    }

    #[test]
    fn format_num_drops_trailing_zeros_for_whole_values() {
        assert_eq!(format_num(3.0), "3");
        assert_eq!(format_num(120.0), "120");
        assert_eq!(format_num(3.5), "3.5");
    }

    #[test]
    fn alloc_returns_usable_memory() {
        __gc_init();
        let p = __alloc(16) as *mut u8;
        assert!(!p.is_null());
        unsafe {
            std::ptr::write_bytes(p, 0xAB, 16);
            assert_eq!(*p, 0xAB);
        }
    }
}
