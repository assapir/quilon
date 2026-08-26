// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Byte-writing intrinsics backing `write`/`print`/`eprint`, plus the shared
//! `write_to_fd` raw-syscall helper the fail-loud paths in `core`/`text` reuse.

use std::os::raw::{c_int, c_void};

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

/// Write `len` bytes from `ptr` to `fd` as human-readable text followed by a newline
/// (backs `print`/`eprint` of a `Text`). The bytes are decoded as UTF-8 with each invalid
/// byte replaced by U+FFFD — `print` renders for a reader, where `__write_bytes` (backing
/// `write`) passes bytes through verbatim.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to at least `len` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __print_text_fd(fd: i64, ptr: *const u8, len: i64) {
    let rendered = crate::text::text_str(ptr, len);
    // Text and newline in one buffer, so one `print` is one write: on a pipe that keeps a
    // line whole against a concurrent writer, and it costs one allocation either way.
    let mut line = Vec::with_capacity(rendered.len() + 1);
    line.extend_from_slice(rendered.as_bytes());
    line.push(b'\n');
    write_to_fd(fd, &line);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `emit` with a fresh pipe as its target descriptor and return the bytes it wrote.
    /// The payloads here are far below a pipe's buffer, so the write never blocks.
    fn captured(emit: impl FnOnce(i64)) -> Vec<u8> {
        use std::io::Read;
        use std::os::fd::AsRawFd;

        let (mut reader, writer) = std::io::pipe().expect("pipe");
        emit(writer.as_raw_fd() as i64);
        drop(writer);

        let mut out = Vec::new();
        reader
            .read_to_end(&mut out)
            .expect("read the captured bytes");
        out
    }

    fn printed(bytes: &[u8]) -> Vec<u8> {
        captured(|fd| __print_text_fd(fd, bytes.as_ptr(), bytes.len() as i64))
    }

    fn written(bytes: &[u8]) -> Vec<u8> {
        captured(|fd| {
            assert_eq!(
                __write_bytes(fd, bytes.as_ptr(), bytes.len() as i64),
                bytes.len() as i64
            );
        })
    }

    #[test]
    fn print_renders_invalid_utf8_as_replacement_while_write_passes_it_through() {
        let bytes = b"a\xffb";
        assert_eq!(written(bytes), bytes);
        assert_eq!(printed(bytes), "a\u{fffd}b\n".as_bytes());
    }

    #[test]
    fn an_interior_nul_survives_both_paths_in_full() {
        let bytes = b"a\0b";
        assert_eq!(written(bytes), bytes);
        assert_eq!(printed(bytes), b"a\0b\n");
    }

    #[test]
    fn print_reads_exactly_len_bytes_of_a_longer_buffer() {
        // The `{ptr,len}` pair is the whole contract: bytes past `len` are not this Text's.
        let buffer = b"visible/hidden";
        assert_eq!(
            captured(|fd| __print_text_fd(fd, buffer.as_ptr(), 7)),
            b"visible\n"
        );
    }

    #[test]
    fn a_null_or_empty_text_prints_just_the_newline() {
        assert_eq!(
            captured(|fd| __print_text_fd(fd, std::ptr::null(), 0)),
            b"\n"
        );
        assert_eq!(printed(b""), b"\n");
    }
}
