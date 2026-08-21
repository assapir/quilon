// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Byte-writing intrinsics backing `write`/`print`/`eprint`, plus the shared
//! `write_to_fd` raw-syscall helper the fail-loud paths in `core`/`text` reuse.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

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
pub(crate) fn write_to_fd(fd: i64, bytes: &[u8]) -> i64 {
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

/// Whether colored output is appropriate on file descriptor `fd`: 1 when it is a terminal
/// and the environment has not opted out, 0 otherwise. Backs the internal
/// `__color_enabled(fd)` primitive, which `core.test` uses to decide whether a failed
/// assertion's report carries ANSI styling.
///
/// Opt-outs, in the order they are checked: `NO_COLOR` set to any non-empty value (the
/// no-color.org convention), `TERM=dumb`, and finally a descriptor that is not a tty (a
/// pipe or a file — which is what keeps captured output plain, in tests included).
#[unsafe(no_mangle)]
pub extern "C" fn __color_enabled(fd: i64) -> i64 {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return 0;
    }
    if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return 0;
    }
    // SAFETY: `isatty` only inspects the descriptor; any value is a defined query (an
    // invalid one answers 0).
    match unsafe { libc::isatty(fd as c_int) } {
        1 => 1,
        _ => 0,
    }
}
