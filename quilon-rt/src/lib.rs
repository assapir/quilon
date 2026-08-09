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
use std::os::raw::{c_char, c_int, c_void};
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

/// Terminate the running program with exit status `code`.
///
/// This is the ONLY native piece of `core.test`: Quilon cannot yet exit/abort
/// mid-program in-language, so the exit primitive lives here as a generic
/// `__exit(code)`. Everything else about assertions — `assert` and its
/// `assertEq`/`assertNotEq`/`assertOk`/`assertNotOk` wrappers — is pure Quilon in
/// `corelib/test.ql`, calling `eprint` for the message and `__exit(101)` to fail.
/// (`101` is the Rust-panic convention `core.test` uses for a failed assertion,
/// deliberately distinct from the small result codes examples use as their normal
/// exit status.) Codegen lowers a `__exit(n)` call to a call of this symbol; see
/// `CodeGenerator::generate_exit`.
///
/// Never returns. Uses libc `exit(3)` directly rather than `std::process::exit` for
/// the same reason `write_to_fd` uses raw `write(2)`: an AOT-linked native binary
/// enters through the LLVM-generated C `main`, so the Rust std runtime is never
/// initialized.
#[unsafe(no_mangle)]
pub extern "C" fn __exit(code: c_int) -> ! {
    // SAFETY: libc `exit` is always available in a linked C runtime; it terminates
    // the process and never returns.
    unsafe extern "C" {
        fn exit(code: c_int) -> !;
    }
    unsafe { exit(code) }
}

/// Report an invalid array index — out of bounds, negative, or NaN — to stderr and
/// terminate with exit status 1: the fail-loud contract of checked `arr[i]` indexing.
/// `index` is the ORIGINAL f64 the program computed (pre-truncation), so the message
/// shows what the user actually asked for; `size` is the array's element count.
/// Codegen calls this from the invalid branch of every `arr[i]` bounds check.
#[unsafe(no_mangle)]
pub extern "C" fn __index_fail(index: f64, size: i64) -> ! {
    let msg = format!(
        "runtime error: array index {} out of bounds (size {})\n",
        format_num(index),
        size
    );
    write_to_fd(2, msg.as_bytes());
    __exit(1)
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

/// Abort on an invalid `replace`/`replaceAll` request (empty `from`, non-positive `count`,
/// or a `count` exceeding the occurrences present): write `msg` + newline to stderr, then
/// terminate the process with exit code 101 via the shared `__exit` intrinsic — the same
/// fail-loud path an assertion failure takes. Never returns. The detection lives in the
/// runtime (not codegen) because the `count > occurrences` case needs the occurrence count.
fn replace_misuse(msg: &str) -> ! {
    write_to_fd(2, msg.as_bytes());
    write_to_fd(2, b"\n");
    __exit(101)
}

/// A Quilon `Text` value (also the representation of an array): `{ ptr data, i64 len }`,
/// matching the code generator's `ptr_len_struct_type` (`{ i8*, i64 }`). For a `Text`,
/// `data` points to `len` UTF-8 bytes; for an array, `data` points to `len` contiguous
/// element-representation values and `len` is the element count. `#[repr(C)]` so the
/// field offsets (ptr at 0, i64 at 8) match what LLVM emits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QlSlice {
    data: *const c_void,
    len: i64,
}

impl QlSlice {
    /// The empty slice (`{ null, 0 }`) — a zero-length `Text`/array. Returned when there
    /// is nothing to build (null/empty `argv`/`envp`).
    fn empty() -> QlSlice {
        QlSlice {
            data: std::ptr::null(),
            len: 0,
        }
    }
}

/// GC-allocate a `Text` whose bytes are a copy of `bytes`. The copy is owned by the GC
/// (so it outlives the C `argv`/`envp` buffers, which the program may not keep), and is
/// NUL-terminated past `len` so `print`/`eprint` (which expect a C string) work too.
fn alloc_text(bytes: &[u8]) -> QlSlice {
    let len = bytes.len();
    // +1 for a trailing NUL so the buffer doubles as a C string for `print`.
    let buf = __alloc(len as i64 + 1) as *mut u8;
    if !buf.is_null() && len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len) };
    }
    QlSlice {
        data: buf as *const c_void,
        len: len as i64,
    }
}

/// Build a Quilon `[]Text` from the C `argc`/`argv` the program's `main` received: one
/// `Text` per argument (including `argv[0]`, the program name), in order. Backs an `^`
/// entry point that declares `args :: []Text`.
///
/// Returns the array as a `{ ptr, i64 }` `QlSlice` whose `data` points to `argc`
/// contiguous `Text` structs — exactly the layout codegen loads for a `[]Text` value.
///
/// # Safety contract (upheld by the C runtime / `main`)
/// `argv` is null, or points to `argc` valid NUL-terminated C strings (the standard
/// `main` contract); a non-positive `argc` yields an empty array.
// Exported C-ABI symbol called from generated code; a safe Rust signature is
// intentional (the contract is upheld by the compiler emitting the call).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __argv_to_text_array(argc: i64, argv: *const *const c_char) -> QlSlice {
    if argv.is_null() || argc <= 0 {
        return QlSlice::empty();
    }
    let n = argc as usize;
    // Allocate the backing array of `n` Text structs (GC-owned).
    let elems = __alloc((n * std::mem::size_of::<QlSlice>()) as i64) as *mut QlSlice;
    for i in 0..n {
        // SAFETY: `argv[0..argc]` are valid C strings per the `main` contract.
        let cstr = unsafe { *argv.add(i) };
        let bytes = cstr_to_str_bytes(cstr);
        let text = alloc_text(&bytes);
        unsafe { std::ptr::write(elems.add(i), text) };
    }
    QlSlice {
        data: elems as *const c_void,
        len: argc,
    }
}

/// Build a Quilon `[][]Text` from the C `envp` the program's `main` received: one inner
/// `[]Text` per environment entry, each an exactly-2-element `[key, value]` split on the
/// FIRST `=`. An entry with no `=` becomes `[entry, ""]` (the whole string as the key,
/// an empty value). Backs an `^` entry point that declares `env :: [][]Text`.
///
/// `envp` is the conventional NULL-terminated array of `key=value` C strings.
///
/// # Safety contract (upheld by the C runtime / `main`)
/// `envp` is null, or points to a NULL-terminated array of valid NUL-terminated C
/// strings (the standard `main`/`environ` contract).
// Exported C-ABI symbol called from generated code; a safe Rust signature is
// intentional (the contract is upheld by the compiler emitting the call).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __envp_to_pairs(envp: *const *const c_char) -> QlSlice {
    if envp.is_null() {
        return QlSlice::empty();
    }
    // First pass: count entries up to the NULL terminator.
    let mut count = 0usize;
    while !unsafe { *envp.add(count) }.is_null() {
        count += 1;
    }
    if count == 0 {
        return QlSlice::empty();
    }
    // Backing array of `count` inner `[]Text` structs (each itself a `QlSlice`).
    let pairs = __alloc((count * std::mem::size_of::<QlSlice>()) as i64) as *mut QlSlice;
    for i in 0..count {
        // SAFETY: i < count, and entries 0..count are non-null per the loop above.
        let cstr = unsafe { *envp.add(i) };
        let bytes = cstr_to_str_bytes(cstr);
        // Split on the FIRST '='; an entry with no '=' is [entry, ""].
        let (key, value): (&[u8], &[u8]) = match bytes.iter().position(|&b| b == b'=') {
            Some(eq) => (&bytes[..eq], &bytes[eq + 1..]),
            None => (&bytes[..], &[]),
        };
        // Inner 2-element `[]Text`: a backing array of exactly two Text structs.
        let kv = __alloc((2 * std::mem::size_of::<QlSlice>()) as i64) as *mut QlSlice;
        unsafe {
            std::ptr::write(kv, alloc_text(key));
            std::ptr::write(kv.add(1), alloc_text(value));
            std::ptr::write(
                pairs.add(i),
                QlSlice {
                    data: kv as *const c_void,
                    len: 2,
                },
            );
        }
    }
    QlSlice {
        data: pairs as *const c_void,
        len: count as i64,
    }
}

// ---------------------------------------------------------------------------
// Text methods — each backs a named, chainable `Text` method, mirroring
// `__text_length` / `__text_cmp`. All are UTF-8 correct and grapheme-based where
// an index/length is user-visible (matching `Text.length`). A `Text` argument
// arrives as `(ptr, len)`; a `Text` / `[]Text` result is returned as a
// GC-allocated `QlSlice` so it outlives this call and is collected like any heap
// value. See `CodeGenerator::get_intrinsic` for the matching prototypes.
// ---------------------------------------------------------------------------

/// Decode `len` bytes at `ptr` as UTF-8 (lossily on invalid UTF-8, which a
/// well-formed Quilon `Text` never is). Shared by all the Text-method intrinsics.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to at least `len` readable bytes.
fn text_str<'a>(ptr: *const u8, len: i64) -> std::borrow::Cow<'a, str> {
    String::from_utf8_lossy(byte_slice(ptr, len))
}

/// Strip leading-only (Unicode) whitespace. Backs `Text.trimStart()`. (`Text.trim()`
/// is composed in codegen as `trimStart` then `trimEnd`, so it needs no own intrinsic.)
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_trim_start(ptr: *const u8, len: i64) -> QlSlice {
    alloc_text(text_str(ptr, len).trim_start().as_bytes())
}

/// Strip trailing-only (Unicode) whitespace. Backs `Text.trimEnd()`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_trim_end(ptr: *const u8, len: i64) -> QlSlice {
    alloc_text(text_str(ptr, len).trim_end().as_bytes())
}

/// Unicode-aware uppercase. Backs `Text.toUpper()`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_to_upper(ptr: *const u8, len: i64) -> QlSlice {
    alloc_text(text_str(ptr, len).to_uppercase().as_bytes())
}

/// Unicode-aware lowercase. Backs `Text.toLower()`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_to_lower(ptr: *const u8, len: i64) -> QlSlice {
    alloc_text(text_str(ptr, len).to_lowercase().as_bytes())
}

/// Whether `sub` occurs in the haystack: 1 (true) / 0 (false). Backs
/// `Text.contains(sub)`. (An empty `sub` is contained in every string.)
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_contains(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> i64 {
    let hay = text_str(hptr, hlen);
    let sub = text_str(sptr, slen);
    i64::from(hay.contains(&*sub))
}

/// The GRAPHEME index of the first occurrence of `sub` in the haystack, or -1 if
/// absent. Backs `Text.indexOf(sub)` — codegen turns -1 into `NotOk` and any other
/// value into `Ok(idx)`. Grapheme-based to match `Text.length` / `Text.slice`; an
/// empty `sub` is found at index 0.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_index_of(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> i64 {
    let hay = text_str(hptr, hlen);
    let sub = text_str(sptr, slen);
    match hay.find(&*sub) {
        // Byte offset -> grapheme index: count the graphemes in the prefix before it.
        Some(byte_idx) => hay[..byte_idx].graphemes(true).count() as i64,
        None => -1,
    }
}

/// Replace EVERY occurrence of `from` with `to`. Backs `Text.replaceAll(from, to)`.
/// An empty `from` is an ill-defined request and ABORTS the process (see `abort_101`).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_replace_all(
    hptr: *const u8,
    hlen: i64,
    fptr: *const u8,
    flen: i64,
    tptr: *const u8,
    tlen: i64,
) -> QlSlice {
    let hay = text_str(hptr, hlen);
    let from = text_str(fptr, flen);
    if from.is_empty() {
        replace_misuse("replace: `from` must not be empty");
    }
    let to = text_str(tptr, tlen);
    alloc_text(hay.replace(&*from, &to).as_bytes())
}

/// Replace EXACTLY the first `count` occurrences of `from` with `to`, left→right. Backs
/// `Text.replace(from, to, count)`. Fails loudly (aborts the process, exit 101) on any
/// invalid input — an empty `from`, a non-positive `count`, or a `count` greater than the
/// number of occurrences actually present (no clamping, no no-op). The checker rejects
/// these at compile time when they are determinable from literals; this is the runtime
/// backstop for computed values.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_replace_n(
    hptr: *const u8,
    hlen: i64,
    fptr: *const u8,
    flen: i64,
    tptr: *const u8,
    tlen: i64,
    count: i64,
) -> QlSlice {
    let hay = text_str(hptr, hlen);
    let from = text_str(fptr, flen);
    // Fail loudly on invalid input — no clamp, no silent no-op (see `replace_misuse`).
    if from.is_empty() {
        replace_misuse("replace: `from` must not be empty");
    }
    if count <= 0 {
        replace_misuse(&format!("replace: count must be positive, got {count}"));
    }
    // Non-overlapping, left→right occurrences — exactly what `replacen` consumes.
    let occurrences = hay.matches(&*from).count() as i64;
    if count > occurrences {
        replace_misuse(&format!(
            "replace: count {count} exceeds {occurrences} occurrences"
        ));
    }
    let to = text_str(tptr, tlen);
    alloc_text(hay.replacen(&*from, &to, count as usize).as_bytes())
}

/// The substring from grapheme `start` (inclusive) to grapheme `end` (exclusive).
/// Indices count graphemes (like `Text.length`); both are CLAMPED to `[0, length]`
/// (never an error), and `end <= start` yields the empty string. Backs `Text.slice`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_slice(ptr: *const u8, len: i64, start: i64, end: i64) -> QlSlice {
    let s = text_str(ptr, len);
    // Byte offset where each grapheme starts, plus a trailing sentinel of `s.len()`, so
    // grapheme index `g` spans bytes `bounds[g]..bounds[g + 1]`. One pass, no String copy;
    // the result is a zero-copy byte subslice that `alloc_text` copies exactly once.
    let mut bounds: Vec<usize> = s.grapheme_indices(true).map(|(b, _)| b).collect();
    bounds.push(s.len());
    let n = (bounds.len() - 1) as i64;
    let clamp = |i: i64| i.clamp(0, n) as usize;
    let (lo, hi) = (clamp(start), clamp(end));
    if hi <= lo {
        return alloc_text(&[]);
    }
    alloc_text(s[bounds[lo]..bounds[hi]].as_bytes())
}

/// Split the haystack on `sep`, returning a `[]Text`. Consecutive separators yield
/// empty pieces (`"a,,b"` -> `["a","","b"]`); an empty haystack yields `[""]`. An
/// EMPTY separator splits into individual graphemes (`"abc"` -> `["a","b","c"]`).
/// Backs `Text.split(sep)`. Returns the array as a `QlSlice` over `len` contiguous
/// `Text` structs — exactly the `[]Text` layout codegen loads.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_split(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> QlSlice {
    let hay = text_str(hptr, hlen);
    let sep = text_str(sptr, slen);
    let parts: Vec<&str> = if sep.is_empty() {
        hay.graphemes(true).collect()
    } else {
        hay.split(&*sep).collect()
    };
    let n = parts.len();
    if n == 0 {
        return QlSlice::empty();
    }
    // Backing array of `n` Text structs (GC-owned), one per piece.
    let elems = __alloc((n * std::mem::size_of::<QlSlice>()) as i64) as *mut QlSlice;
    for (i, part) in parts.iter().enumerate() {
        // SAFETY: `elems` has room for `n` `QlSlice`s and `i < n`.
        unsafe { std::ptr::write(elems.add(i), alloc_text(part.as_bytes())) };
    }
    QlSlice {
        data: elems as *const c_void,
        len: n as i64,
    }
}

/// The bytes of a NUL-terminated C string (empty for null). Used to copy `argv`/`envp`
/// entries into GC-owned `Text`s.
fn cstr_to_str_bytes(ptr: *const c_char) -> Vec<u8> {
    if ptr.is_null() {
        return Vec::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
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
static QUILON_RT_INTRINSICS: [RtFn; 22] = unsafe {
    [
        core::mem::transmute(__gc_init as extern "C" fn()),
        core::mem::transmute(__exit as extern "C" fn(c_int) -> !),
        core::mem::transmute(__index_fail as extern "C" fn(f64, i64) -> !),
        core::mem::transmute(__alloc as extern "C" fn(i64) -> *mut c_void),
        core::mem::transmute(__text_length as extern "C" fn(*const u8, i64) -> i64),
        core::mem::transmute(__text_cmp as extern "C" fn(*const u8, i64, *const u8, i64) -> i32),
        core::mem::transmute(__write_bytes as extern "C" fn(i64, *const u8, i64) -> i64),
        core::mem::transmute(__print_num_fd as extern "C" fn(i64, f64)),
        core::mem::transmute(__print_bool_fd as extern "C" fn(i64, i64)),
        core::mem::transmute(__print_text_fd as extern "C" fn(i64, *const c_char)),
        core::mem::transmute(
            __argv_to_text_array as extern "C" fn(i64, *const *const c_char) -> QlSlice,
        ),
        core::mem::transmute(__envp_to_pairs as extern "C" fn(*const *const c_char) -> QlSlice),
        core::mem::transmute(__text_trim_start as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_trim_end as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_to_upper as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_to_lower as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(
            __text_contains as extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
        ),
        core::mem::transmute(
            __text_index_of as extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
        ),
        core::mem::transmute(
            __text_replace_all
                as extern "C" fn(*const u8, i64, *const u8, i64, *const u8, i64) -> QlSlice,
        ),
        core::mem::transmute(
            __text_replace_n
                as extern "C" fn(*const u8, i64, *const u8, i64, *const u8, i64, i64) -> QlSlice,
        ),
        core::mem::transmute(__text_slice as extern "C" fn(*const u8, i64, i64, i64) -> QlSlice),
        core::mem::transmute(
            __text_split as extern "C" fn(*const u8, i64, *const u8, i64) -> QlSlice,
        ),
    ]
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // libgc's `GC_init`/`GC_malloc` are not safe to invoke from several threads at
    // once; cargo runs tests in parallel, so every test that initializes/allocates
    // through the GC takes this lock first (mirrors `jit`'s JIT_LOCK).
    static GC_LOCK: Mutex<()> = Mutex::new(());

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

    /// View a `QlSlice` `Text` result as a `&str` (its GC-owned bytes). Takes the
    /// `QlSlice` by value (it is `Copy`) so the returned `&str` borrows the underlying
    /// GC buffer, not the (temporary) struct.
    unsafe fn slice_str<'a>(s: QlSlice) -> &'a str {
        let bytes = unsafe { std::slice::from_raw_parts(s.data as *const u8, s.len as usize) };
        std::str::from_utf8(bytes).unwrap()
    }

    fn text_of(s: &str) -> (*const u8, i64) {
        (s.as_ptr(), s.len() as i64)
    }

    #[test]
    fn text_trim_start_and_end() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // Unicode whitespace (NBSP U+00A0, EM SPACE U+2003) on both ends.
        let (p, l) = text_of("\u{00A0}\u{2003}héllo\u{2003}\u{00A0}");
        assert_eq!(
            unsafe { slice_str(__text_trim_start(p, l)) },
            "héllo\u{2003}\u{00A0}"
        );
        assert_eq!(
            unsafe { slice_str(__text_trim_end(p, l)) },
            "\u{00A0}\u{2003}héllo"
        );
    }

    #[test]
    fn text_case_mapping_is_unicode() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (p, l) = text_of("Straße");
        assert_eq!(unsafe { slice_str(__text_to_upper(p, l)) }, "STRASSE");
        let (p, l) = text_of("HÉLLO");
        assert_eq!(unsafe { slice_str(__text_to_lower(p, l)) }, "héllo");
    }

    #[test]
    fn text_contains_and_index_of_are_grapheme_based() {
        let (hp, hl) = text_of("héllo");
        let (sp, sl) = text_of("llo");
        assert_eq!(__text_contains(hp, hl, sp, sl), 1);
        // "llo" starts after "hé" — 2 graphemes in, even though "é" is 2 bytes.
        assert_eq!(__text_index_of(hp, hl, sp, sl), 2);
        let (zp, zl) = text_of("z");
        assert_eq!(__text_contains(hp, hl, zp, zl), 0);
        assert_eq!(__text_index_of(hp, hl, zp, zl), -1);
        // An empty needle is contained at index 0.
        let (ep, el) = text_of("");
        assert_eq!(__text_index_of(hp, hl, ep, el), 0);
    }

    // NOTE: the fail-loud paths (empty `from`, count <= 0, count > occurrences) abort the
    // process via `abort_101`, so they CANNOT be exercised from an in-process unit test —
    // they would exit the test runner. They are covered by subprocess tests in
    // tests/text_methods_test.rs (which run `quilon run` and assert exit 101). Here we only
    // test the valid inputs.
    #[test]
    fn text_replace_n_valid_counts() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (hp, hl) = text_of("a-a-a");
        let (fp, fl) = text_of("a");
        let (tp, tl) = text_of("xx");
        // count = 1 -> first only.
        assert_eq!(
            unsafe { slice_str(__text_replace_n(hp, hl, fp, fl, tp, tl, 1)) },
            "xx-a-a"
        );
        // count = 2 -> first two.
        assert_eq!(
            unsafe { slice_str(__text_replace_n(hp, hl, fp, fl, tp, tl, 2)) },
            "xx-xx-a"
        );
        // count == exact number of occurrences -> all three.
        assert_eq!(
            unsafe { slice_str(__text_replace_n(hp, hl, fp, fl, tp, tl, 3)) },
            "xx-xx-xx"
        );
    }

    #[test]
    fn text_replace_all_replaces_every_occurrence() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (hp, hl) = text_of("a-a-a");
        let (fp, fl) = text_of("a");
        let (tp, tl) = text_of("xx");
        assert_eq!(
            unsafe { slice_str(__text_replace_all(hp, hl, fp, fl, tp, tl)) },
            "xx-xx-xx"
        );
    }

    #[test]
    fn text_slice_clamps_and_counts_graphemes() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (p, l) = text_of("héllo"); // 5 graphemes
        assert_eq!(unsafe { slice_str(__text_slice(p, l, 1, 4)) }, "éll");
        assert_eq!(unsafe { slice_str(__text_slice(p, l, -5, 100)) }, "héllo"); // clamp
        assert_eq!(unsafe { slice_str(__text_slice(p, l, 3, 1)) }, ""); // end<=start
    }

    #[test]
    fn text_split_variants() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let collect = |s: &QlSlice| -> Vec<String> {
            let parts =
                unsafe { std::slice::from_raw_parts(s.data as *const QlSlice, s.len as usize) };
            parts
                .iter()
                .map(|p| unsafe { slice_str(*p) }.to_string())
                .collect()
        };
        let (hp, hl) = text_of("a,,b");
        let (sp, sl) = text_of(",");
        assert_eq!(collect(&__text_split(hp, hl, sp, sl)), ["a", "", "b"]);
        // Empty haystack -> a single empty piece.
        let (ep, el) = text_of("");
        assert_eq!(collect(&__text_split(ep, el, sp, sl)), [""]);
        // Empty separator -> graphemes.
        let (gp, gl) = text_of("héllo");
        let (esp, esl) = text_of("");
        assert_eq!(
            collect(&__text_split(gp, gl, esp, esl)),
            ["h", "é", "l", "l", "o"]
        );
    }

    /// A single collect helper shared by the Unicode split tests below.
    fn split_parts(s: &QlSlice) -> Vec<String> {
        let parts = unsafe { std::slice::from_raw_parts(s.data as *const QlSlice, s.len as usize) };
        parts
            .iter()
            .map(|p| unsafe { slice_str(*p) }.to_string())
            .collect()
    }

    #[test]
    fn text_slice_does_not_split_multi_codepoint_graphemes() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // "e" + combining acute (U+0301) is ONE grapheme but 3 bytes; slicing grapheme
        // [0,1) must return the whole cluster, never half of it.
        let combining = "e\u{0301}llo"; // "éllo" in NFD: 5 graphemes, 6 bytes
        let (p, l) = text_of(combining);
        assert_eq!(unsafe { slice_str(__text_slice(p, l, 0, 1)) }, "e\u{0301}");
        assert_eq!(unsafe { slice_str(__text_slice(p, l, 0, 1)) }.len(), 3);
        // A ZWJ family emoji is one grapheme; slice [0,1) keeps it intact.
        let (fp, fl) = text_of("👨‍👩‍👧x");
        assert_eq!(unsafe { slice_str(__text_slice(fp, fl, 0, 1)) }, "👨‍👩‍👧");
        // An emoji mid-string: "a🌍b" graphemes a(0) 🌍(1) b(2).
        let (ep, el) = text_of("a🌍b");
        assert_eq!(unsafe { slice_str(__text_slice(ep, el, 1, 2)) }, "🌍");
    }

    #[test]
    fn text_index_of_and_contains_are_grapheme_correct_on_multibyte() {
        // "a🌍b": 🌍 is 4 bytes / 1 grapheme; "b" is at grapheme index 2, byte offset 5.
        let (hp, hl) = text_of("a🌍b");
        let (bp, bl) = text_of("b");
        assert_eq!(__text_index_of(hp, hl, bp, bl), 2);
        let (ep, el) = text_of("🌍");
        assert_eq!(__text_index_of(hp, hl, ep, el), 1);
        assert_eq!(__text_contains(hp, hl, ep, el), 1);
        // 🌎 (U+1F30E) shares its first 3 UTF-8 bytes with 🌍 (U+1F30D) but differs in the
        // last — a byte-overlapping-but-different substring must NOT falsely match.
        let (fp, fl) = text_of("🌎");
        assert_eq!(__text_contains(hp, hl, fp, fl), 0);
        assert_eq!(__text_index_of(hp, hl, fp, fl), -1);
    }

    #[test]
    fn text_split_on_multibyte_separator_and_cluster_stays_whole() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // Split on a 4-byte emoji separator.
        let (hp, hl) = text_of("a🌍b🌍c");
        let (sp, sl) = text_of("🌍");
        assert_eq!(split_parts(&__text_split(hp, hl, sp, sl)), ["a", "b", "c"]);
        // Empty-separator split of a multi-codepoint grapheme keeps it as ONE element.
        let (fp, fl) = text_of("👨‍👩‍👧");
        let (esp, esl) = text_of("");
        assert_eq!(split_parts(&__text_split(fp, fl, esp, esl)), ["👨‍👩‍👧"]);
    }

    #[test]
    fn text_case_mapping_non_ascii_and_one_to_many() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (p, l) = text_of("é");
        assert_eq!(unsafe { slice_str(__text_to_upper(p, l)) }, "É");
        let (p, l) = text_of("Ä");
        assert_eq!(unsafe { slice_str(__text_to_lower(p, l)) }, "ä");
        // 1->N case mapping: German sharp-s uppercases to "SS" (documented Rust behavior).
        let (p, l) = text_of("ß");
        assert_eq!(unsafe { slice_str(__text_to_upper(p, l)) }, "SS");
        let (p, l) = text_of("İ"); // U+0130 LATIN CAPITAL I WITH DOT ABOVE
        assert_eq!(unsafe { slice_str(__text_to_lower(p, l)) }, "i\u{307}");
    }

    #[test]
    fn text_trim_composed_strips_unicode_whitespace_both_sides() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // `trim` is `trimStart` then `trimEnd` (as codegen composes it). NBSP (U+00A0)
        // and EM SPACE (U+2003) are Unicode whitespace and must be stripped from both ends.
        let (p, l) = text_of("\u{00A0}\u{2003}héllo\u{2003}\u{00A0}");
        let started = __text_trim_start(p, l);
        let trimmed = __text_trim_end(started.data as *const u8, started.len);
        assert_eq!(unsafe { slice_str(trimmed) }, "héllo");
    }

    #[test]
    fn text_replace_with_multibyte_from_and_to() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (hp, hl) = text_of("a🌍b🌍c");
        let (fp, fl) = text_of("🌍");
        let (tp, tl) = text_of("é");
        // replaceAll: both 4-byte emoji separators become the 2-byte "é".
        assert_eq!(
            unsafe { slice_str(__text_replace_all(hp, hl, fp, fl, tp, tl)) },
            "aébéc"
        );
        // replace count = 1: only the first.
        assert_eq!(
            unsafe { slice_str(__text_replace_n(hp, hl, fp, fl, tp, tl, 1)) },
            "aéb🌍c"
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
