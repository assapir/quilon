// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! HTTP response body framing — the byte-level work `core.http`'s `Response.body()` and
//! `Request.send()` cannot do in Quilon. A chunk boundary and a `Content-Length` count both
//! cut BYTES, while `Text` slices by GRAPHEME, so a server that ends a chunk inside a
//! multi-byte character has no Quilon-reachable split point (the body would carry a
//! replacement character where the boundary fell). [`__http_frame_body`] does the whole
//! byte-level job instead: locate the head/body blank line, then dechunk, take exactly
//! `Content-Length` bytes, or close-delimit. Head parsing (status line, headers) stays
//! Quilon, in `corelib/http.qn`.

use crate::deferred::QlResult;

/// The four spellings of a blank line `Response.blankLine()` measures on the SAME bytes,
/// earliest match wins.
const BLANK_LINE_SPELLINGS: [&[u8]; 4] = [b"\r\n\r\n", b"\n\n", b"\r\n\n", b"\n\r\n"];

/// Where `raw`'s head/body blank line begins and how many bytes it spans, or `None` when
/// `raw` carries no blank line at all.
fn find_blank_line(raw: &[u8]) -> Option<(usize, usize)> {
    BLANK_LINE_SPELLINGS
        .iter()
        .filter_map(|spelling| find_subslice(raw, spelling).map(|at| (at, spelling.len())))
        .min_by_key(|&(at, _)| at)
}

/// The earliest index at which `needle` occurs in `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Whether `transfer_encoding` names `chunked` as its LAST, case-insensitive,
/// comma-separated token — the encoding actually applied closest to the wire.
fn is_chunked(transfer_encoding: &str) -> bool {
    transfer_encoding
        .rsplit(',')
        .next()
        .is_some_and(|token| token.trim().eq_ignore_ascii_case("chunked"))
}

/// Dechunk `body`: a hex size line (an optional `;extension` ignored), exactly that many
/// data bytes, a terminating CRLF, repeated; a `0` chunk ends the data, and any trailers up
/// to the final blank line are dropped without being parsed. A non-hex size, a missing
/// CRLF, or data ending early is malformed framing.
fn dechunk(body: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut decoded = Vec::new();
    let mut rest = body;
    loop {
        let line_end = find_subslice(rest, b"\r\n").ok_or("malformed chunked framing")?;
        let size_field = &rest[..line_end];
        let size_hex = match size_field.iter().position(|&byte| byte == b';') {
            Some(at) => &size_field[..at],
            None => size_field,
        };
        let size_text = std::str::from_utf8(size_hex).map_err(|_| "malformed chunked framing")?;
        let size =
            usize::from_str_radix(size_text.trim(), 16).map_err(|_| "malformed chunked framing")?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if rest.len() < size + 2 || &rest[size..size + 2] != b"\r\n" {
            return Err("malformed chunked framing");
        }
        decoded.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

/// The framing rule `core.http` applies to one reply's body, given what the head already
/// established. A `bodiless` reply (1xx/204/304, or any HEAD reply) carries no body
/// regardless of what its headers claim; `chunked` (once the blank line is found) takes
/// precedence over `Content-Length`; with neither, the close delimits the body. Pure bytes
/// throughout — a server may cut a chunk or a `Content-Length` count in the middle of a
/// multi-byte character, so nothing here ever decodes as UTF-8.
fn frame_body(
    raw: &[u8],
    bodiless: bool,
    transfer_encoding: &str,
    content_length: &str,
) -> QlResult {
    let Some((at, spelling_len)) = find_blank_line(raw) else {
        return QlResult::ok(&[]);
    };
    let body = &raw[at + spelling_len..];
    if bodiless {
        return QlResult::ok(&[]);
    }
    if is_chunked(transfer_encoding) {
        return match dechunk(body) {
            Ok(decoded) => QlResult::ok(&decoded),
            Err(reason) => QlResult::not_ok(reason),
        };
    }
    if !content_length.is_empty() {
        return match content_length.trim().parse::<usize>() {
            Ok(expected) if body.len() >= expected => QlResult::ok(&body[..expected]),
            Ok(expected) => QlResult::not_ok(&format!(
                "truncated body: expected {expected} bytes, got {}",
                body.len()
            )),
            Err(_) => QlResult::not_ok("malformed Content-Length"),
        };
    }
    QlResult::ok(body)
}

/// `core.http.frameBody(raw, bodiless, transferEncoding, contentLength) -> Result`: the
/// native body-framing intrinsic behind `Response.body()` and `Request.send()`'s framing
/// check. Synchronous — unlike `@tcpRequest` there is no IO here, only parsing bytes
/// already in hand — so it writes straight into `out` rather than through the deferred-value
/// machinery `launch`/`force` share.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QlResult`]; each `(*_data, *_len)` pair is
/// null with a non-positive length, or points to `*_len` readable bytes for this call (the
/// `Text` arguments' live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __http_frame_body(
    out: *mut QlResult,
    raw_data: *const u8,
    raw_len: i64,
    bodiless: i8,
    transfer_encoding_data: *const u8,
    transfer_encoding_len: i64,
    content_length_data: *const u8,
    content_length_len: i64,
) {
    let raw = borrow_bytes(raw_data, raw_len);
    let transfer_encoding = text_of(transfer_encoding_data, transfer_encoding_len);
    let content_length = text_of(content_length_data, content_length_len);
    let result = frame_body(raw, bodiless != 0, &transfer_encoding, &content_length);
    // SAFETY: `out` is writable storage for one `QlResult` (the code generator's alloca).
    unsafe { *out = result };
}

/// Borrow a `Text`'s `len` content bytes at `data`, past its header.
fn borrow_bytes<'a>(data: *const u8, len: i64) -> &'a [u8] {
    crate::text::byte_slice(data, len)
}

/// Copy `len` bytes at `data` into an owned `String` (empty if null/empty/invalid UTF-8) — a
/// header value, which the checker already knows is `Text`.
///
/// # Safety contract (upheld by the compiler)
/// `data` is null, or points to `len` readable bytes for the duration of this call.
fn text_of(data: *const u8, len: i64) -> String {
    String::from_utf8_lossy(borrow_bytes(data, len)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::{RESULT_NOTOK_TAG, RESULT_OK_TAG};
    use crate::mem::__gc_init;
    use crate::test_support::GC_LOCK;

    /// `__http_frame_body` allocates its `Ok`/`NotOk` payload through the GC (`alloc_text`),
    /// so a test calling it needs the collector initialized and — since cargo runs tests in
    /// parallel on their own OS threads — serialized against every other GC-touching test
    /// (mirrors `mem.rs`'s own `__alloc` tests).
    fn frame(
        raw: &[u8],
        bodiless: bool,
        transfer_encoding: &str,
        content_length: &str,
    ) -> (i8, Vec<u8>) {
        let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let mut out = QlResult {
            tag: 0,
            slot: crate::mem::QlSlice::empty(),
        };
        let (raw_ptr, raw_len) = crate::test_support::text_of_bytes(raw);
        let (transfer_encoding_ptr, transfer_encoding_len) =
            crate::test_support::text_of(transfer_encoding);
        let (content_length_ptr, content_length_len) = crate::test_support::text_of(content_length);
        __http_frame_body(
            &mut out,
            raw_ptr,
            raw_len,
            bodiless as i8,
            transfer_encoding_ptr,
            transfer_encoding_len,
            content_length_ptr,
            content_length_len,
        );
        let bytes = crate::text::byte_slice(out.slot.data as *const u8, out.slot.len).to_vec();
        (out.tag, bytes)
    }

    #[test]
    fn takes_everything_after_the_blank_line_with_no_framing_header() {
        let (tag, body) = frame(b"HTTP/1.1 200 OK\r\nX-A: b\r\n\r\npayload", false, "", "");
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"payload");
    }

    #[test]
    fn empty_body_when_there_is_no_blank_line() {
        let (tag, body) = frame(b"HTTP/1.1 200 OK\r\nX-A: b\r\n", false, "", "");
        assert_eq!(tag, RESULT_OK_TAG);
        assert!(body.is_empty());
    }

    #[test]
    fn bodiless_ignores_a_content_length() {
        let (tag, body) = frame(
            b"HTTP/1.1 204 No Content\r\nContent-Length: 1234\r\n\r\n",
            true,
            "",
            "1234",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert!(body.is_empty());
    }

    #[test]
    fn content_length_takes_exactly_that_many_bytes() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\npayload",
            false,
            "",
            "2",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"pa");
    }

    #[test]
    fn content_length_short_of_the_bytes_present_is_not_ok() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\npayload",
            false,
            "",
            "999",
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(body, b"truncated body: expected 999 bytes, got 7");
    }

    #[test]
    fn non_numeric_content_length_is_not_ok() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\npayload",
            false,
            "",
            "abc",
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(body, b"malformed Content-Length");
    }

    #[test]
    fn dechunks_an_ascii_reply() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            false,
            "chunked",
            "",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn dechunks_a_multi_byte_character_split_across_a_chunk_boundary() {
        // "héllo" is 'h' (1 byte), 'é' (2 bytes, 0xC3 0xA9), "llo" (3 bytes). The first chunk
        // ends between the two bytes of 'é' — the whole reason this framing is native: a
        // byte-level split has no Quilon-reachable position (Text slices by grapheme).
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        raw.extend_from_slice(b"2\r\nh\xC3\r\n4\r\n\xA9llo\r\n0\r\n\r\n");
        let (tag, body) = frame(&raw, false, "chunked", "");
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(String::from_utf8(body).unwrap(), "héllo");
    }

    #[test]
    fn dechunks_ignoring_chunk_extensions() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;foo=bar\r\nhello\r\n0\r\n\r\n",
            false,
            "chunked",
            "",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn dechunks_dropping_trailers() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\nX-Trailer: t\r\n\r\n",
            false,
            "chunked",
            "",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn chunked_takes_precedence_over_a_stray_content_length() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 999\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            false,
            "chunked",
            "999",
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn a_non_hex_chunk_size_is_malformed() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nhello\r\n0\r\n\r\n",
            false,
            "chunked",
            "",
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(body, b"malformed chunked framing");
    }

    #[test]
    fn a_chunk_shorter_than_its_declared_size_is_malformed() {
        let (tag, body) = frame(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhi\r\n0\r\n\r\n",
            false,
            "chunked",
            "",
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(body, b"malformed chunked framing");
    }

    #[test]
    fn transfer_encoding_names_chunked_as_its_last_token() {
        assert!(is_chunked("chunked"));
        assert!(is_chunked("gzip, chunked"));
        assert!(is_chunked("GZIP, CHUNKED"));
        assert!(!is_chunked("chunked, gzip"));
        assert!(!is_chunked("identity"));
        assert!(!is_chunked(""));
    }
}
