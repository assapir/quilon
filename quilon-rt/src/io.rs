// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Byte-writing intrinsics backing `write`/`print`/`eprint`, the shared `write_to_fd`
//! raw-syscall helper the fail-loud paths in `core`/`text` reuse, and `@streamFile` — the
//! chunk-callback file read.

use crate::deferred::{QlResult, read_once, set_nonblocking};
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
use std::os::raw::{c_int, c_void};
use std::os::unix::io::AsRawFd;
use unicode_segmentation::UnicodeSegmentation;

/// The symbolic name a report shows for an errno this runtime's writes can plausibly hit —
/// a closed reader (`EPIPE`), a descriptor with no write end (`EBADF`), a full disk
/// (`ENOSPC`), or a reset connection (`ECONNRESET`). Anything else falls back to its raw
/// number, which is still enough for a reader to look up.
fn errno_name(errno: i32) -> String {
    match errno {
        libc::EPIPE => "EPIPE".to_string(),
        libc::EBADF => "EBADF".to_string(),
        libc::EIO => "EIO".to_string(),
        libc::ENOSPC => "ENOSPC".to_string(),
        libc::ECONNRESET => "ECONNRESET".to_string(),
        other => format!("errno {other}"),
    }
}

/// A `write` file descriptor as the non-negative whole number it must be, or the message
/// saying why it is not.
fn check_write_fd(fd: f64) -> Result<i32, String> {
    if fd.fract() != 0.0 || fd < 0.0 || fd > i32::MAX as f64 {
        return Err(format!(
            "write: file descriptor must be a whole number of 0 or more, got {}",
            crate::mem::format_num(fd)
        ));
    }
    Ok(fd as i32)
}

/// Write a rendered `Text`'s `len` content bytes (past its header, at `ptr`) to file
/// descriptor `fd`, returning the number of bytes written (0 on null/empty/error). Backs
/// the `write(content, fd)` builtin — a descriptor that is not a whole number of 0 or more
/// is refused loud, reported at `site` (the call's own location).
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points at a header `quilon-rt` wrote, followed by `len` readable
/// bytes; `site` is null or points to a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __write_bytes(fd: f64, ptr: *const u8, len: i64, site: *const QlSite) -> i64 {
    let fd = match check_write_fd(fd) {
        Ok(fd) => fd,
        Err(message) => fail_at(site, codes::WRITE_FD_NOT_WHOLE, &message, RUNTIME_EXIT_CODE),
    };
    let bytes = crate::text::byte_slice(ptr, len);
    match write_to_fd(fd as i64, bytes) {
        Ok(n) => n,
        Err(message) => fail_at(site, codes::WRITE_FAILED, &message, RUNTIME_EXIT_CODE),
    }
}

/// Write `len` bytes from `ptr` to `fd` as human-readable text followed by a newline
/// (backs `print`/`eprint` of a `Text`). The bytes are decoded as UTF-8 with each invalid
/// byte replaced by U+FFFD — `print` renders for a reader, where `__write_bytes` (backing
/// `write`) passes bytes through verbatim.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to at least `len` readable bytes; `site` is null or points to
/// a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __print_text_fd(fd: i64, ptr: *const u8, len: i64, site: *const QlSite) {
    let rendered = crate::text::text_str(ptr, len);
    // Text and newline in one buffer, so one `print` is one write: on a pipe that keeps a
    // line whole against a concurrent writer, and it costs one allocation either way.
    let mut line = Vec::with_capacity(rendered.len() + 1);
    line.extend_from_slice(rendered.as_bytes());
    line.push(b'\n');
    if let Err(message) = write_to_fd(fd, &line) {
        fail_at(site, codes::WRITE_FAILED, &message, RUNTIME_EXIT_CODE);
    }
}

/// Write all `bytes` to descriptor `fd` without closing it, retrying a partial write and an
/// interrupted one (`EINTR`) until every byte is placed. Returns bytes written, or a message
/// naming the errno on any other failure — a closed reader (`EPIPE`) chief among them, since
/// a native binary ignores `SIGPIPE` at startup (see `__gc_init`) so that failure reaches here
/// as an error instead of a signal.
///
/// Uses libc `write(2)` directly rather than `std::fs::File`. AOT-linked native
/// binaries enter through the LLVM-generated C `main`, so the Rust std runtime is
/// never initialized and std's higher-level I/O does not work there — a raw
/// syscall does, and resolves identically under the JIT.
pub(crate) fn write_to_fd(fd: i64, bytes: &[u8]) -> Result<i64, String> {
    if bytes.is_empty() {
        return Ok(0);
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
        if n < 0 {
            let error = std::io::Error::last_os_error();
            let errno = error.raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue;
            }
            // `io::Error`'s own message ends in " (os error N)"; swap that suffix for the
            // symbolic name a reader actually recognizes.
            let message = error.to_string();
            let message = message
                .strip_suffix(&format!(" (os error {errno})"))
                .unwrap_or(&message);
            return Err(format!("write failed: {message} ({})", errno_name(errno)));
        }
        if n == 0 {
            break;
        }
        total += n as usize;
    }
    Ok(total as i64)
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

/// `@streamFile(path, chunkSize, onChunk)`: read `path` in `chunkSize`-byte reads, calling the
/// bundled Quilon closure once per whole, valid-Text chunk — STRICTLY, on the calling fiber
/// (unlike `@readStdin`/`@tcpRequest`'s launch-and-defer): it runs in program order and only
/// parks (via [`read_once`]) on reactor readiness between reads, so other fibers still overlap.
/// `on_chunk`/`environment` are the code generator's fixed-shape trampoline over the closure
/// (see `CodeGenerator::emit_stream_file_thunk`) and the bundled `{ptr,ptr}` closure value it
/// unpacks. Writes `Ok(bytesRead)` (total bytes delivered) into `out` at EOF or once `onChunk`
/// returns `false`, or `NotOk(message)` on any failure — never fails the process.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QlResult`]; `path_data` is null, or points to
/// `path_len` readable bytes; `on_chunk` is the function pointer of a live `(ptr,i64,ptr)->i8`
/// trampoline, called with `environment` as its last argument.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __stream_file_run(
    out: *mut QlResult,
    path_data: *const u8,
    path_len: i64,
    chunk_size: f64,
    on_chunk: *const c_void,
    environment: *mut c_void,
) {
    // SAFETY: per the contract, a live `(ptr,i64,ptr) -> i8` trampoline.
    let on_chunk: extern "C" fn(*const u8, i64, *mut c_void) -> u8 =
        unsafe { std::mem::transmute(on_chunk) };
    let path = crate::text::text_str(path_data, path_len).into_owned();
    let result = stream_file(&path, chunk_size, on_chunk, environment);
    // SAFETY: `out` is writable storage for one `QlResult` (the code generator's alloca).
    unsafe { *out = result };
}

/// Call the bundled Quilon closure with one chunk's bytes (built into a proper `Text`, header
/// included, via [`crate::mem::alloc_text`] — so `chunk.length` counts its graphemes correctly),
/// returning whether it asked to keep reading (`true`) or stop (`false`).
fn call_on_chunk(
    on_chunk: extern "C" fn(*const u8, i64, *mut c_void) -> u8,
    environment: *mut c_void,
    bytes: &[u8],
) -> bool {
    let text = crate::mem::alloc_text(bytes);
    on_chunk(text.data as *const u8, text.len, environment) != 0
}

/// `chunkSize` as the positive whole number of bytes it must be, or the message saying why
/// it is not.
fn check_chunk_size(chunk_size: f64) -> Result<usize, String> {
    if chunk_size.fract() != 0.0 || chunk_size <= 0.0 {
        return Err(format!(
            "@streamFile: chunkSize must be a positive whole number, got {}",
            crate::mem::format_num(chunk_size)
        ));
    }
    Ok(chunk_size as usize)
}

/// The `@streamFile` read loop: open `path`, read it in `chunk_size`-byte reads (parking on
/// reactor readiness between them via [`read_once`]), and call `on_chunk` once per whole,
/// valid-Text chunk.
///
/// Every chunk handed to `on_chunk` is a whole grapheme cluster prefix: an incomplete UTF-8
/// sequence or an incomplete grapheme cluster at the end of a read is held back and prepended
/// to the next read, UNLESS the file's own size (read once, at open) says nothing more will
/// ever arrive — then everything gathered so far is delivered as one final chunk, without
/// holding anything back. That size check is skipped (the conservative, always-hold-back path
/// runs instead, until a `read_once` genuinely returns `0`) for a `path` metadata calls this
/// on that is not a plain file, or when metadata fails.
fn stream_file(
    path: &str,
    chunk_size: f64,
    on_chunk: extern "C" fn(*const u8, i64, *mut c_void) -> u8,
    environment: *mut c_void,
) -> QlResult {
    let chunk_size = match check_chunk_size(chunk_size) {
        Ok(size) => size,
        Err(message) => return QlResult::not_ok(&message),
    };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return QlResult::not_ok(&format!("@streamFile failed to open {path}: {error}"));
        }
    };
    let known_size = file
        .metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| metadata.len());
    let fd = file.as_raw_fd();
    set_nonblocking(fd);

    let mut carry: Vec<u8> = Vec::new();
    let mut read_so_far: u64 = 0;
    let mut delivered: i64 = 0;
    let mut buffer = vec![0u8; chunk_size];

    loop {
        let count = match read_once(fd, &mut buffer) {
            Ok(count) => count,
            Err(error) => {
                return QlResult::not_ok(&format!("@streamFile failed to read {path}: {error}"));
            }
        };
        if count == 0 {
            // True EOF: whatever remains held is delivered as-is, per the documented contract.
            if !carry.is_empty() {
                delivered += carry.len() as i64;
                call_on_chunk(on_chunk, environment, &carry);
            }
            return QlResult::ok_num(delivered as f64);
        }
        read_so_far += count as u64;
        carry.extend_from_slice(&buffer[..count]);

        if known_size.is_some_and(|size| read_so_far >= size) {
            // Everything the file held when it was opened has now been read: nothing more
            // will ever arrive, so nothing needs holding back.
            delivered += carry.len() as i64;
            call_on_chunk(on_chunk, environment, &carry);
            return QlResult::ok_num(delivered as f64);
        }

        let (deliverable, tail) = match split_chunk(&carry) {
            Ok(pair) => pair,
            Err(message) => return QlResult::not_ok(&message),
        };
        carry = tail;
        if deliverable.is_empty() {
            continue;
        }
        delivered += deliverable.len() as i64;
        if !call_on_chunk(on_chunk, environment, &deliverable) {
            return QlResult::ok_num(delivered as f64);
        }
    }
}

/// Split `buffer` into the whole, valid-Text prefix ready to deliver and the tail bytes to
/// carry into the next read: an incomplete UTF-8 sequence at the end, then the last grapheme
/// cluster of what remains — more bytes could still extend either, so both stay held back.
/// Errors on genuinely invalid UTF-8 (as opposed to merely an incomplete trailing sequence).
fn split_chunk(buffer: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let valid_len = match std::str::from_utf8(buffer) {
        Ok(_) => buffer.len(),
        Err(error) => match error.error_len() {
            None => error.valid_up_to(),
            Some(_) => return Err("@streamFile read bytes that are not valid UTF-8".to_string()),
        },
    };
    let valid = std::str::from_utf8(&buffer[..valid_len]).expect("checked above");
    let cut = valid
        .grapheme_indices(true)
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0);
    Ok((buffer[..cut].to_vec(), buffer[cut..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::text_of_bytes;

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
        let (ptr, len) = text_of_bytes(bytes);
        captured(|fd| __print_text_fd(fd, ptr, len, std::ptr::null()))
    }

    fn written(bytes: &[u8]) -> Vec<u8> {
        let (ptr, len) = text_of_bytes(bytes);
        captured(|fd| {
            assert_eq!(
                __write_bytes(fd as f64, ptr, len, std::ptr::null()),
                bytes.len() as i64
            );
        })
    }

    #[test]
    fn a_whole_non_negative_fd_converts() {
        assert_eq!(check_write_fd(0.0), Ok(0));
        assert_eq!(check_write_fd(7.0), Ok(7));
        assert_eq!(check_write_fd(i32::MAX as f64), Ok(i32::MAX));
    }

    #[test]
    fn a_malformed_fd_is_refused() {
        for (fd, shown) in [
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::INFINITY, "inf"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (i32::MAX as f64 + 1.0, "2147483648"),
        ] {
            let message = check_write_fd(fd).expect_err("must be refused");
            assert_eq!(
                message,
                format!("write: file descriptor must be a whole number of 0 or more, got {shown}")
            );
        }
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
        let (ptr, _) = text_of_bytes(b"visible/hidden");
        assert_eq!(
            captured(|fd| __print_text_fd(fd, ptr, 7, std::ptr::null())),
            b"visible\n"
        );
    }

    #[test]
    fn a_null_or_empty_text_prints_just_the_newline() {
        assert_eq!(
            captured(|fd| __print_text_fd(fd, std::ptr::null(), 0, std::ptr::null())),
            b"\n"
        );
        assert_eq!(printed(b""), b"\n");
    }
}
