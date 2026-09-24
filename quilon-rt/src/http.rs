// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! HTTP body framing — the byte-level work `core.http`'s `Response.body()`,
//! `Request.send()`, and the server's own `serveConnection` cannot do in Quilon. A chunk
//! boundary and a `Content-Length` count both cut BYTES, while `Text` slices by GRAPHEME,
//! so a server that ends a chunk inside a multi-byte character has no Quilon-reachable
//! split point (the body would carry a replacement character where the boundary fell).
//! [`__http_frame_body`] frames a REPLY already fully in hand (the client waits for the
//! peer to close before calling it); [`__http_body_progress`] answers the same framing
//! question incrementally for a REQUEST body still arriving, so `corelib/http.qn`'s own
//! `readBody` knows when to stop reading. Head parsing (status/request line, headers)
//! stays Quilon either way.

use crate::deferred::QnResult;

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

/// The three things [`dechunk_progress`]/[`body_progress`] can determine about a REQUEST
/// body still arriving, unlike [`frame_body`]'s reply — which always sees the whole thing,
/// since the client waits for the peer to close before framing anything: framed already
/// (`Complete`), more bytes needed (`Incomplete`), too big for the server's own cap
/// (`TooLarge`), or broken beyond repair (`Malformed`). [`dechunk`] reuses the same
/// state machine with an unbounded cap, where `Incomplete`/`TooLarge` fold into the one
/// error a reply already fully in hand has no room to distinguish them from.
enum BodyProgress {
    Complete(Vec<u8>),
    Incomplete,
    TooLarge,
    Malformed(&'static str),
}

/// Dechunk a hex size line (an optional `;extension` ignored), exactly that many data
/// bytes, a terminating CRLF, repeated; a `0` chunk ends the data, and any trailers up to
/// the final blank line are dropped without being parsed. Stops the moment the decoded
/// total would exceed `max_body_size` — before waiting for a chunk bigger than the whole
/// cap to finish arriving — and reports data that has not finished arriving yet
/// (`Incomplete`) apart from data that will never parse (`Malformed`).
fn dechunk_progress(body: &[u8], max_body_size: usize) -> BodyProgress {
    let mut decoded = Vec::new();
    let mut rest = body;
    loop {
        let Some(line_end) = find_subslice(rest, b"\r\n") else {
            return BodyProgress::Incomplete;
        };
        let size_field = &rest[..line_end];
        let size_hex = match size_field.iter().position(|&byte| byte == b';') {
            Some(at) => &size_field[..at],
            None => size_field,
        };
        let Ok(size_text) = std::str::from_utf8(size_hex) else {
            return BodyProgress::Malformed("malformed chunked framing");
        };
        let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
            return BodyProgress::Malformed("malformed chunked framing");
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            return BodyProgress::Complete(decoded);
        }
        // `checked_add`, not `decoded.len() + size`: a crafted size near `usize::MAX` (a
        // valid hex parse) would otherwise wrap past `max_body_size` undetected, and the
        // `size + 2` below would wrap into a slice that panics rather than reporting
        // `TooLarge`. Either overflow already means "bigger than any real cap".
        let over_cap = match decoded.len().checked_add(size) {
            Some(total) => total > max_body_size,
            None => true,
        };
        if over_cap {
            return BodyProgress::TooLarge;
        }
        let Some(chunk_and_terminator) = size.checked_add(2) else {
            return BodyProgress::TooLarge;
        };
        if rest.len() < chunk_and_terminator {
            return BodyProgress::Incomplete;
        }
        if &rest[size..chunk_and_terminator] != b"\r\n" {
            return BodyProgress::Malformed("malformed chunked framing");
        }
        decoded.extend_from_slice(&rest[..size]);
        rest = &rest[chunk_and_terminator..];
    }
}

/// Dechunk a reply already fully in hand: [`dechunk_progress`] with no cap
/// (`usize::MAX`), so `TooLarge` cannot occur except from the same size-overflow
/// [`dechunk_progress`] itself treats as malformed, and `Incomplete` — data that would
/// finish arriving given more bytes, which a reply already fully received never will —
/// means the framing was truncated, exactly as malformed as anything else `dechunk`
/// itself reports.
fn dechunk(body: &[u8]) -> Result<Vec<u8>, &'static str> {
    match dechunk_progress(body, usize::MAX) {
        BodyProgress::Complete(decoded) => Ok(decoded),
        BodyProgress::Incomplete | BodyProgress::TooLarge => Err("malformed chunked framing"),
        BodyProgress::Malformed(reason) => Err(reason),
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
) -> QnResult {
    let Some((at, spelling_len)) = find_blank_line(raw) else {
        return QnResult::ok(&[]);
    };
    let body = &raw[at + spelling_len..];
    if bodiless {
        return QnResult::ok(&[]);
    }
    if is_chunked(transfer_encoding) {
        return match dechunk(body) {
            Ok(decoded) => QnResult::ok(&decoded),
            Err(reason) => QnResult::not_ok(reason),
        };
    }
    if !content_length.is_empty() {
        return match content_length.trim().parse::<usize>() {
            Ok(expected) if body.len() >= expected => QnResult::ok(&body[..expected]),
            Ok(expected) => QnResult::not_ok(&format!(
                "truncated body: expected {expected} bytes, got {}",
                body.len()
            )),
            Err(_) => QnResult::not_ok("malformed Content-Length"),
        };
    }
    QnResult::ok(body)
}

/// The framing rule `core.http`'s connection handler applies to a REQUEST body still being
/// read, given `raw` (the connection's own bytes received so far, from the very first
/// one): `chunked` takes precedence over `Content-Length`, mirroring [`frame_body`]'s own
/// rule. Never called with neither header present — a method with no meaning for a body,
/// or one given neither header, has no body to read at all, decided in `corelib/http.qn`
/// before this runs.
fn body_progress(
    raw: &[u8],
    transfer_encoding: &str,
    content_length: &str,
    max_body_size: usize,
) -> BodyProgress {
    let Some((at, spelling_len)) = find_blank_line(raw) else {
        return BodyProgress::Incomplete;
    };
    let body = &raw[at + spelling_len..];
    if is_chunked(transfer_encoding) {
        return dechunk_progress(body, max_body_size);
    }
    // A `Transfer-Encoding` present but not (ending in) `chunked` names an encoding this
    // server does not decode — distinct from `content_length` simply being malformed, so
    // the reason names what is actually wrong rather than blaming an absent header.
    if !transfer_encoding.is_empty() {
        return BodyProgress::Malformed("unsupported Transfer-Encoding");
    }
    match content_length.trim().parse::<usize>() {
        Ok(expected) if expected > max_body_size => BodyProgress::TooLarge,
        Ok(expected) if body.len() >= expected => BodyProgress::Complete(body[..expected].to_vec()),
        Ok(_) => BodyProgress::Incomplete,
        Err(_) => BodyProgress::Malformed("malformed Content-Length"),
    }
}

/// `core.http.bodyProgress(accumulated, transferEncoding, contentLength, maxBodySize) ->
/// Result`: the native primitive behind `serveConnection`'s own body-reading loop
/// (`readBody`/`answer` in `corelib/http.qn`). `Ok(bodyBytes)` once the framing
/// `transferEncoding`/`contentLength` declare has fully arrived; `NotOk("incomplete")`
/// while more bytes are still needed; `NotOk("too large")` once the declared or decoded
/// size passes `maxBodySize`; `NotOk(reason)` for any other malformed framing.
/// Synchronous, like `__http_frame_body` — the bytes are already in hand, there is no IO
/// here.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QnResult`]; each `(*_data, *_len)` pair is
/// null with a non-positive length, or points to `*_len` readable bytes for this call (the
/// `Text` arguments' live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __http_body_progress(
    out: *mut QnResult,
    raw_data: *const u8,
    raw_len: i64,
    transfer_encoding_data: *const u8,
    transfer_encoding_len: i64,
    content_length_data: *const u8,
    content_length_len: i64,
    max_body_size: f64,
) {
    let raw = borrow_bytes(raw_data, raw_len);
    let transfer_encoding = text_of(transfer_encoding_data, transfer_encoding_len);
    let content_length = text_of(content_length_data, content_length_len);
    // `as usize` saturates rather than overflows or panics — the same conversion
    // `core.io.@streamFile`'s own chunk size takes (`quilon-rt/src/io.rs`).
    let max_body_size = max_body_size as usize;
    let result = match body_progress(raw, &transfer_encoding, &content_length, max_body_size) {
        BodyProgress::Complete(bytes) => QnResult::ok(&bytes),
        BodyProgress::Incomplete => QnResult::not_ok("incomplete"),
        BodyProgress::TooLarge => QnResult::not_ok("too large"),
        BodyProgress::Malformed(reason) => QnResult::not_ok(reason),
    };
    // SAFETY: `out` is writable storage for one `QnResult` (the code generator's alloca).
    unsafe { *out = result };
}

/// `core.http.frameBody(raw, bodiless, transferEncoding, contentLength) -> Result`: the
/// native body-framing intrinsic behind `Response.body()` and `Request.send()`'s framing
/// check. Synchronous — unlike `@tcpRequest` there is no IO here, only parsing bytes
/// already in hand — so it writes straight into `out` rather than through the deferred-value
/// machinery `launch`/`force` share.
///
/// # Safety contract (upheld by the compiler)
/// `out` points to writable storage for one [`QnResult`]; each `(*_data, *_len)` pair is
/// null with a non-positive length, or points to `*_len` readable bytes for this call (the
/// `Text` arguments' live bytes at the call site).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __http_frame_body(
    out: *mut QnResult,
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
    // SAFETY: `out` is writable storage for one `QnResult` (the code generator's alloca).
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
        let mut out = QnResult {
            tag: 0,
            slot: crate::mem::QnSlice::empty(),
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

    /// `__http_body_progress`'s own harness, mirroring `frame` above: a `NotOk` payload is
    /// read back as TEXT (`"incomplete"`, `"too large"`, or a malformed-framing reason)
    /// rather than raw bytes, since that is what `readBody`/`answer` themselves match on.
    fn progress(
        raw: &[u8],
        transfer_encoding: &str,
        content_length: &str,
        max_body_size: f64,
    ) -> (i8, String) {
        let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let mut out = QnResult {
            tag: 0,
            slot: crate::mem::QnSlice::empty(),
        };
        let (raw_ptr, raw_len) = crate::test_support::text_of_bytes(raw);
        let (transfer_encoding_ptr, transfer_encoding_len) =
            crate::test_support::text_of(transfer_encoding);
        let (content_length_ptr, content_length_len) = crate::test_support::text_of(content_length);
        __http_body_progress(
            &mut out,
            raw_ptr,
            raw_len,
            transfer_encoding_ptr,
            transfer_encoding_len,
            content_length_ptr,
            content_length_len,
            max_body_size,
        );
        let bytes = crate::text::byte_slice(out.slot.data as *const u8, out.slot.len).to_vec();
        (out.tag, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn progress_is_incomplete_before_the_head_s_blank_line_has_arrived() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nContent-Length: 5",
            "",
            "5",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "incomplete");
    }

    #[test]
    fn progress_is_incomplete_short_of_a_declared_content_length() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nContent-Length: 5\r\n\r\nbe",
            "",
            "5",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "incomplete");
    }

    #[test]
    fn progress_completes_once_content_length_bytes_have_all_arrived() {
        let (tag, body) = progress(
            b"POST /orders HTTP/1.1\r\nContent-Length: 5\r\n\r\nbeans",
            "",
            "5",
            1024.0,
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, "beans");
    }

    #[test]
    fn progress_rejects_a_declared_content_length_over_the_cap_before_any_body_arrives() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nContent-Length: 999\r\n\r\n",
            "",
            "999",
            10.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "too large");
    }

    #[test]
    fn progress_is_malformed_for_a_non_numeric_content_length() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nContent-Length: abc\r\n\r\nx",
            "",
            "abc",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "malformed Content-Length");
    }

    #[test]
    fn progress_is_incomplete_mid_chunk() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel",
            "chunked",
            "",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "incomplete");
    }

    #[test]
    fn progress_completes_at_the_zero_size_chunk() {
        let (tag, body) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            "chunked",
            "",
            1024.0,
        );
        assert_eq!(tag, RESULT_OK_TAG);
        assert_eq!(body, "hello");
    }

    #[test]
    fn progress_rejects_a_chunk_whose_declared_size_would_exceed_the_cap() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n64\r\n",
            "chunked",
            "",
            10.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "too large");
    }

    #[test]
    fn progress_is_malformed_for_a_non_hex_chunk_size_once_the_size_line_is_complete() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nhello\r\n0\r\n\r\n",
            "chunked",
            "",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "malformed chunked framing");
    }

    /// A chunk-size line near `usize::MAX` (a valid hex parse) must read as `"too large"`
    /// rather than wrapping the cap check and panicking on the slice that follows it.
    #[test]
    fn progress_rejects_a_chunk_size_near_usize_max_without_panicking() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nfffffffffffffffe\r\n",
            "chunked",
            "",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "too large");
    }

    #[test]
    fn dechunk_rejects_a_chunk_size_near_usize_max_without_panicking() {
        let result = dechunk(b"fffffffffffffffe\r\nhello\r\n0\r\n\r\n");
        assert_eq!(result, Err("malformed chunked framing"));
    }

    #[test]
    fn progress_is_malformed_for_an_unsupported_transfer_encoding() {
        let (tag, reason) = progress(
            b"POST /orders HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
            "gzip",
            "",
            1024.0,
        );
        assert_eq!(tag, RESULT_NOTOK_TAG);
        assert_eq!(reason, "unsupported Transfer-Encoding");
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
