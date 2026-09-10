// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Text intrinsics — the PRIMITIVE floor under the built-in `Text` methods:
//! segmentation (`length`, `graphemes`, `at`), comparison (`cmp`), the
//! whitespace walks (`trimStart`/`trimEnd`), case mapping, substring search
//! (`indexOf`), grapheme-boundary `slice`, and the byte-linear walks
//! (`split`, `replaceAll`, `replace`). The remaining composable methods
//! (`trim`/`contains`/`repeat`) are written in Quilon over these
//! (`corelib/text.qn`), so they are deliberately NOT here.
//! All are UTF-8 correct and grapheme-based where an index/length is
//! user-visible (matching `Text.length`). A `Text` argument arrives as
//! `(ptr, len)`; a `Text` / `[]Text` result is returned as a GC-allocated
//! `QlSlice` so it outlives this call and is collected like any heap value. See
//! `CodeGenerator::get_intrinsic` for the matching prototypes.

use crate::mem::{
    GRAPHEME_MERGE_FLOOR, QlSlice, TEXT_HEADER_BYTES, alloc_slots, alloc_text, alloc_text_buffer,
    alloc_text_with_count, format_num, inherited_flags, text_header_of, text_is_ascii,
    text_is_valid_utf8, text_no_bidi_controls,
};
use crate::report::{QlSite, RUNTIME_EXIT_CODE, codes, fail_at};
use std::os::raw::c_void;
use unicode_segmentation::UnicodeSegmentation;

/// Render a `Num` to a GC-allocated `Text` (the built-in `` ` `` for Num, and the shared
/// render path for string interpolation and `print`). Whole values render without a
/// fractional part (`5`, not `5.0`); other values use the shortest round-trip form.
#[unsafe(no_mangle)]
pub extern "C" fn __num_to_text(x: f64) -> QlSlice {
    alloc_text(format_num(x).as_bytes())
}

/// Render a `Bool` to a GC-allocated `Text` (the built-in `` ` `` for Bool): `True` /
/// `False`, capitalized — deliberately distinct from the lowercase `true`/`false`
/// literals. `b` is the bool zero-extended to an integer (0 = false).
#[unsafe(no_mangle)]
pub extern "C" fn __bool_to_text(b: i64) -> QlSlice {
    alloc_text(if b != 0 { b"True" } else { b"False" })
}

/// Backs `Text.length`: O(1), reading the header rather than walking. `ptr` is the
/// `Text`'s own `data` field (the header), not its bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_length(ptr: *const u8, len: i64) -> i64 {
    if len <= 0 {
        return 0;
    }
    text_header_of(ptr).0
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

/// A `Text`'s `len` content bytes, past its header (empty for a null/non-positive `len`).
pub(crate) fn byte_slice<'a>(ptr: *const u8, len: i64) -> &'a [u8] {
    if ptr.is_null() || len <= 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr.add(TEXT_HEADER_BYTES as usize), len as usize) }
    }
}

/// Decode `len` bytes at `ptr` as UTF-8, skipping validation when the header's valid-UTF-8
/// bit already answers it (lossily decoded otherwise). Shared by all the Text-method
/// intrinsics.
pub(crate) fn text_str<'a>(ptr: *const u8, len: i64) -> std::borrow::Cow<'a, str> {
    let bytes = byte_slice(ptr, len);
    if text_is_valid_utf8(text_header_of(ptr).1) {
        // SAFETY: the header's valid-UTF-8 bit guarantees `bytes` is valid UTF-8.
        std::borrow::Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(bytes) })
    } else {
        String::from_utf8_lossy(bytes)
    }
}

/// Strip leading-only (Unicode) whitespace. Backs `Text.trimStart()`. (`Text.trim()`
/// composes the two walks in `core.text`, so it needs no own intrinsic.)
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

/// Whether `sub` occurs in the haystack: 1 (true) / 0 (false). Backs the `contains`
/// ASSERTION matcher (`assert(x, contains(sub))`) — the compiler lowers that check here
/// directly. The `Text.contains` METHOD is Quilon (`core.text`), over `indexOf`.
/// (An empty `sub` is contained in every string.)
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_contains(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> i64 {
    let hay = text_str(hptr, hlen);
    let sub = text_str(sptr, slen);
    i64::from(hay.contains(&*sub))
}

/// Shared by [`__text_index_of`] (`from = 0`) and [`__text_index_of_from`]; on an
/// ASCII-aligned haystack a byte offset doubles as its own grapheme index, so both ends
/// of the search skip the grapheme walk.
fn text_find_from(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64, from: i64) -> i64 {
    let (count, flags) = text_header_of(hptr);
    let ascii = text_is_ascii(flags);
    let hay = text_str(hptr, hlen);
    let sub = text_str(sptr, slen);
    let from_byte = if ascii {
        from.clamp(0, hlen) as usize
    } else {
        hay.grapheme_indices(true)
            .nth(from.clamp(0, count) as usize)
            .map_or(hay.len(), |(byte_idx, _)| byte_idx)
    };
    match hay[from_byte..].find(&*sub) {
        Some(relative) => {
            let byte_idx = from_byte + relative;
            if ascii {
                byte_idx as i64
            } else {
                hay[..byte_idx].graphemes(true).count() as i64
            }
        }
        None => -1,
    }
}

/// The GRAPHEME index of the first occurrence of `sub` in the haystack, or -1 if
/// absent. Backs `Text.indexOf(sub)` — codegen turns -1 into `NotOk` and any other
/// value into `Ok(idx)`. Grapheme-based to match `Text.length` / `Text.slice`; an
/// empty `sub` is found at index 0.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_index_of(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> i64 {
    text_find_from(hptr, hlen, sptr, slen, 0)
}

/// Backs `Text.indexOf(sub, from)`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_index_of_from(
    hptr: *const u8,
    hlen: i64,
    sptr: *const u8,
    slen: i64,
    from: i64,
) -> i64 {
    text_find_from(hptr, hlen, sptr, slen, from)
}

/// The substring from grapheme `start` (inclusive) to grapheme `end` (exclusive).
/// Indices count graphemes (like `Text.length`); both are CLAMPED to `[0, length]`
/// (never an error), and `end <= start` yields the empty string. Backs `Text.slice`. On
/// an ASCII-aligned receiver a byte offset doubles as its own grapheme index, so the
/// whole call is a byte-range copy with no grapheme walk.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_slice(ptr: *const u8, len: i64, start: i64, end: i64) -> QlSlice {
    let (_, flags) = text_header_of(ptr);
    if text_is_ascii(flags) {
        let clamp = |i: i64| i.clamp(0, len);
        let (lo, hi) = (clamp(start), clamp(end));
        if hi <= lo {
            return QlSlice::empty();
        }
        let bytes = byte_slice(ptr, len);
        return alloc_text_with_count(&bytes[lo as usize..hi as usize], hi - lo);
    }
    let s = text_str(ptr, len);
    // Byte offset where each grapheme starts, plus a trailing sentinel of `s.len()`, so
    // grapheme index `g` spans bytes `bounds[g]..bounds[g + 1]`.
    let mut bounds: Vec<usize> = s.grapheme_indices(true).map(|(b, _)| b).collect();
    bounds.push(s.len());
    let n = (bounds.len() - 1) as i64;
    let clamp = |i: i64| i.clamp(0, n) as usize;
    let (lo, hi) = (clamp(start), clamp(end));
    if hi <= lo {
        return QlSlice::empty();
    }
    alloc_text_with_count(s[bounds[lo]..bounds[hi]].as_bytes(), (hi - lo) as i64)
}

/// Build a `[]Text` (a `QlSlice` over `parts.len()` contiguous `Text` structs — the
/// layout codegen loads) with one length-`parts[i].len()` `Text` per slice, each
/// GC-allocated. Shared by every native primitive that answers with an array of pieces
/// (`graphemes`, `split`).
fn text_array(parts: &[&str]) -> QlSlice {
    if parts.is_empty() {
        return QlSlice::empty();
    }
    let elems = alloc_slots::<QlSlice>(parts.len());
    for (i, part) in parts.iter().enumerate() {
        // SAFETY: `elems` has room for `parts.len()` `QlSlice`s and `i < parts.len()`.
        unsafe { std::ptr::write(elems.add(i), alloc_text(part.as_bytes())) };
    }
    QlSlice {
        data: elems as *const c_void,
        len: parts.len() as i64,
    }
}

/// The individual graphemes of the text, as a `[]Text` of length-1 `Text`s — the
/// segmentation primitive `Text = []Grapheme` rests on. Backs `Text.graphemes()` and
/// `Text.split("")` (an empty separator, delegated to this from [`__text_split`]). An
/// empty text has no graphemes: `[]`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_graphemes(ptr: *const u8, len: i64) -> QlSlice {
    let s = text_str(ptr, len);
    let parts: Vec<&str> = s.graphemes(true).collect();
    text_array(&parts)
}

/// The grapheme at `index` (0-based), or the EMPTY text when `index` is out of bounds —
/// a grapheme is never empty, so codegen reads the empty answer as `NotOk`. Backs
/// `Text.at(index)`. On an ASCII-aligned receiver, `index` is its own byte offset.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_at(ptr: *const u8, len: i64, index: i64) -> QlSlice {
    if index < 0 {
        return QlSlice::empty();
    }
    let (_, flags) = text_header_of(ptr);
    if text_is_ascii(flags) {
        if index >= len {
            return QlSlice::empty();
        }
        let bytes = byte_slice(ptr, len);
        return alloc_text_with_count(&bytes[index as usize..index as usize + 1], 1);
    }
    let s = text_str(ptr, len);
    match s.graphemes(true).nth(index as usize) {
        Some(grapheme) => alloc_text_with_count(grapheme.as_bytes(), 1),
        None => QlSlice::empty(),
    }
}

/// Split the haystack on every non-overlapping occurrence of `sep`, as a `[]Text` —
/// consecutive separators keep the pieces between them empty, and an empty haystack with
/// a non-empty `sep` yields a single empty piece. An empty `sep` splits into individual
/// graphemes instead (delegating to [`__text_graphemes`]). Backs `Text.split(sep)`.
///
/// The separator is matched on raw bytes, not re-derived from grapheme indices: a
/// grapheme boundary is always a byte boundary in valid UTF-8, so a byte-level match can
/// never land inside one, and the search stays linear in the haystack's length.
///
/// # Safety contract (upheld by the compiler)
/// `hptr`/`sptr` are null or point to at least `hlen`/`slen` readable bytes.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_split(hptr: *const u8, hlen: i64, sptr: *const u8, slen: i64) -> QlSlice {
    let sep = text_str(sptr, slen);
    if sep.is_empty() {
        return __text_graphemes(hptr, hlen);
    }
    let hay = text_str(hptr, hlen);
    let parts: Vec<&str> = hay.split(&*sep).collect();
    text_array(&parts)
}

/// Replace every non-overlapping occurrence of `from` with `to`, left to right. Backs
/// `Text.replaceAll(from, to)`, matched on raw bytes for the same reason [`__text_split`]
/// is: a byte-level match cannot land inside a grapheme cluster.
///
/// An empty `from` is an ill-defined request: report it at `site` (the method call's own
/// location) and exit the way a failing `assert` does. A literal empty `from` is instead
/// rejected at compile time (`check_replace_literals`), so only a COMPUTED one reaches
/// this check.
///
/// # Safety contract (upheld by the compiler)
/// `hptr`/`fptr`/`tptr` are null or point to at least `hlen`/`flen`/`tlen` readable
/// bytes; `site` is null or points to a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_replace_all(
    hptr: *const u8,
    hlen: i64,
    fptr: *const u8,
    flen: i64,
    tptr: *const u8,
    tlen: i64,
    site: *const QlSite,
) -> QlSlice {
    let from = text_str(fptr, flen);
    if from.is_empty() {
        fail_at(
            site,
            codes::REPLACE_EMPTY_FROM,
            "replaceAll: `from` must not be empty",
            RUNTIME_EXIT_CODE,
        );
    }
    let hay = text_str(hptr, hlen);
    let to = text_str(tptr, tlen);
    alloc_text(hay.replace(&*from, &to).as_bytes())
}

/// [`__text_replace`]'s pure half: EXACTLY the first `count` occurrences of `from` in
/// `hay`, replaced by `to`, left to right; `count` truncates toward zero. `Err` names the
/// runtime code and message for an ill-defined request — an empty `from`, a `count` that
/// truncates to less than 1, or a `count` past the occurrences `from` actually has (no
/// clamp, no no-op) — split out so the three failures are testable without a `QlSite` or
/// `fail_at`'s process exit.
fn replace_text(hay: &str, from: &str, to: &str, count: f64) -> Result<String, (u16, String)> {
    if from.is_empty() {
        return Err((
            codes::REPLACE_EMPTY_FROM,
            "replace: `from` must not be empty".to_string(),
        ));
    }
    let count = count.trunc();
    if count.is_nan() || count < 1.0 {
        return Err((
            codes::REPLACE_INVALID_COUNT,
            format!("replace: count must be positive, got {}", format_num(count)),
        ));
    }
    let occurrences = hay.matches(from).count();
    if count > occurrences as f64 {
        return Err((
            codes::REPLACE_INVALID_COUNT,
            format!(
                "replace: count {} exceeds {occurrences} occurrences",
                format_num(count)
            ),
        ));
    }
    Ok(hay.replacen(from, to, count as usize))
}

/// Backs `Text.replace(from, to, count)`, matched on raw bytes for the same reason
/// [`__text_replace_all`] is. A literal violation of [`replace_text`]'s contract is
/// instead a compile-time error (`check_replace_literals`), so only a COMPUTED one
/// reaches this check.
///
/// # Safety contract (upheld by the compiler)
/// `hptr`/`fptr`/`tptr` are null or point to at least `hlen`/`flen`/`tlen` readable
/// bytes; `site` is null or points to a valid [`QlSite`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_replace(
    hptr: *const u8,
    hlen: i64,
    fptr: *const u8,
    flen: i64,
    tptr: *const u8,
    tlen: i64,
    count: f64,
    site: *const QlSite,
) -> QlSlice {
    let (hay, from, to) = (
        text_str(hptr, hlen),
        text_str(fptr, flen),
        text_str(tptr, tlen),
    );
    match replace_text(&hay, &from, &to, count) {
        Ok(result) => alloc_text(result.as_bytes()),
        Err((code, message)) => fail_at(site, code, &message, RUNTIME_EXIT_CODE),
    }
}

/// The grapheme count and ASCII-aligned bit for `left ++ right`, correcting for a
/// grapheme that spans the seam — GB3's CR×LF, a combining mark attaching backward,
/// regional-indicator pairing — that each side's own header can't see across on its own.
/// `left`/`right` are each side's decoded content.
///
/// A boundary char below [`GRAPHEME_MERGE_FLOOR`] (and not the CR×LF pair) can never
/// merge with its neighbor, so `chars().next_back()`/`next()` — O(1) on UTF-8, since a
/// char decodes from at most 4 bytes at either end — answer the overwhelmingly common
/// case with no grapheme work at all; only a boundary at or above the floor takes the
/// window path (a combining mark, ZWJ, variation selector, or regional indicator).
fn seam_corrected(
    left: &str,
    l_count: i64,
    l_ascii: bool,
    right: &str,
    r_count: i64,
    r_ascii: bool,
) -> (i64, bool) {
    if left.is_empty() {
        return (r_count, r_ascii);
    }
    if right.is_empty() {
        return (l_count, l_ascii);
    }
    let last_char = left.chars().next_back().unwrap_or('\0');
    let first_char = right.chars().next().unwrap_or('\0');
    if last_char < GRAPHEME_MERGE_FLOOR
        && first_char < GRAPHEME_MERGE_FLOOR
        && !(last_char == '\r' && first_char == '\n')
    {
        return (l_count + r_count, l_ascii && r_ascii);
    }
    let left_last = left.graphemes(true).next_back().unwrap_or("");
    let right_first = right.graphemes(true).next().unwrap_or("");
    let mut window = String::with_capacity(left_last.len() + right_first.len());
    window.push_str(left_last);
    window.push_str(right_first);
    let window_count = window.graphemes(true).count() as i64;
    let count = l_count + r_count - 2 + window_count;
    let ascii = l_ascii && r_ascii && !(left_last == "\r" && right_first == "\n");
    (count, ascii)
}

/// [`seam_corrected`], from a `Text`'s own header pointer/length rather than pre-decoded
/// content — the fast path both `+` and `join` take: two ASCII-aligned operands never
/// carry a `\r` at all (`is_ascii_grapheme_aligned`), so their seam can never merge and the
/// counts add exactly, with no decode.
fn concat_header(
    left: (*const u8, i64, i64, bool),
    right: (*const u8, i64, i64, bool),
) -> (i64, bool) {
    let (lptr, llen, l_count, l_ascii) = left;
    let (rptr, rlen, r_count, r_ascii) = right;
    if llen <= 0 {
        return (r_count, r_ascii);
    }
    if rlen <= 0 {
        return (l_count, l_ascii);
    }
    if l_ascii && r_ascii {
        return (l_count + r_count, true);
    }
    let left = text_str(lptr, llen);
    let right = text_str(rptr, rlen);
    seam_corrected(&left, l_count, l_ascii, &right, r_count, r_ascii)
}

/// Backs `Text` `+`. No walk on the (overwhelmingly common) ASCII-aligned path; one
/// allocation either way, filled by two direct copies (no intermediate buffer).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_concat(lptr: *const u8, llen: i64, rptr: *const u8, rlen: i64) -> QlSlice {
    let total = llen.max(0) + rlen.max(0);
    if total <= 0 {
        return QlSlice::empty();
    }
    let (l_count, l_flags) = text_header_of(lptr);
    let (r_count, r_flags) = text_header_of(rptr);
    let l_ascii = llen <= 0 || text_is_ascii(l_flags);
    let r_ascii = rlen <= 0 || text_is_ascii(r_flags);
    let (count, ascii) = concat_header(
        (lptr, llen, l_count, l_ascii),
        (rptr, rlen, r_count, r_ascii),
    );
    // ASCII-aligned text is always free of bidi controls by construction
    // (`is_ascii_grapheme_aligned` sets both bits together), so the overwhelmingly common
    // ASCII+ASCII path already has its answer; only a non-ASCII operand needs its bit read.
    let no_bidi = ascii
        || ((llen <= 0 || text_no_bidi_controls(l_flags))
            && (rlen <= 0 || text_no_bidi_controls(r_flags)));
    let (slice, content) =
        alloc_text_buffer(total as usize, count, inherited_flags(ascii, no_bidi));
    let l_bytes = byte_slice(lptr, llen);
    let r_bytes = byte_slice(rptr, rlen);
    // SAFETY: `content` has room for exactly `l_bytes.len() + r_bytes.len()` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(l_bytes.as_ptr(), content, l_bytes.len());
        std::ptr::copy_nonoverlapping(r_bytes.as_ptr(), content.add(l_bytes.len()), r_bytes.len());
    }
    slice
}

/// [`seam_corrected`], folded left to right over `[]Text.join`'s pieces and separators —
/// the same seam a chain of `+` would see at each junction. Only reached when at least one
/// piece or the separator is not ASCII-aligned (`__text_join`'s own fast path otherwise).
fn join_count_with_seams(
    parts: &[QlSlice],
    sep_ptr: *const u8,
    sep_len: i64,
    sep_count: i64,
) -> i64 {
    let mut acc: Option<(i64, String)> = None;
    let mut fold = |text: &str, count: i64| {
        if text.is_empty() {
            return;
        }
        acc = Some(match acc.take() {
            None => (
                count,
                text.graphemes(true).next_back().unwrap_or("").to_string(),
            ),
            Some((acc_count, acc_trailing)) => {
                // Same O(1) gate as `seam_corrected`: a boundary char below
                // GRAPHEME_MERGE_FLOOR can never merge with its neighbor.
                let left_last_char = acc_trailing.chars().next_back().unwrap_or('\0');
                let right_first_char = text.chars().next().unwrap_or('\0');
                if left_last_char < GRAPHEME_MERGE_FLOOR
                    && right_first_char < GRAPHEME_MERGE_FLOOR
                    && !(left_last_char == '\r' && right_first_char == '\n')
                {
                    (
                        acc_count + count,
                        text.graphemes(true).next_back().unwrap_or("").to_string(),
                    )
                } else {
                    let right_first = text.graphemes(true).next().unwrap_or("");
                    let mut window = String::with_capacity(acc_trailing.len() + right_first.len());
                    window.push_str(&acc_trailing);
                    window.push_str(right_first);
                    let window_count = window.graphemes(true).count() as i64;
                    let new_count = acc_count + count - 2 + window_count;
                    let new_trailing = if window_count == 1 {
                        window
                    } else {
                        text.graphemes(true).next_back().unwrap_or("").to_string()
                    };
                    (new_count, new_trailing)
                }
            }
        });
    };
    for (i, part) in parts.iter().enumerate() {
        let (part_count, _) = text_header_of(part.data as *const u8);
        fold(&text_str(part.data as *const u8, part.len), part_count);
        if i + 1 < parts.len() {
            fold(&text_str(sep_ptr, sep_len), sep_count);
        }
    }
    acc.map_or(0, |(count, _)| count)
}

/// Backs `[]Text.join(separator)`. `parts_ptr` is the array ABI's `data` field:
/// `parts_len` contiguous `Text` structs. One allocation, filled by a direct copy per
/// piece and separator (no intermediate buffer).
///
/// # Safety contract (upheld by the compiler)
/// `parts_ptr` is null or points at `parts_len` contiguous, readable `Text` structs.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_join(
    parts_ptr: *const c_void,
    parts_len: i64,
    sep_ptr: *const u8,
    sep_len: i64,
) -> QlSlice {
    if parts_ptr.is_null() || parts_len <= 0 {
        return QlSlice::empty();
    }
    // SAFETY: upheld by the caller (see the contract above).
    let parts =
        unsafe { std::slice::from_raw_parts(parts_ptr as *const QlSlice, parts_len as usize) };
    let sep_bytes = byte_slice(sep_ptr, sep_len);
    let (sep_count, sep_flags) = text_header_of(sep_ptr);
    let sep_ascii = sep_bytes.is_empty() || text_is_ascii(sep_flags);
    let sep_no_bidi = sep_bytes.is_empty() || text_no_bidi_controls(sep_flags);

    let mut total_len: i64 = 0;
    let mut total_count: i64 = 0;
    let mut ascii = true;
    let mut no_bidi = true;
    for (i, part) in parts.iter().enumerate() {
        let part_bytes = byte_slice(part.data as *const u8, part.len);
        let (count, flags) = text_header_of(part.data as *const u8);
        total_len += part_bytes.len() as i64;
        total_count += count;
        ascii &= part_bytes.is_empty() || text_is_ascii(flags);
        no_bidi &= part_bytes.is_empty() || text_no_bidi_controls(flags);
        if i + 1 < parts.len() {
            total_len += sep_bytes.len() as i64;
            total_count += sep_count;
            ascii &= sep_ascii;
            no_bidi &= sep_no_bidi;
        }
    }
    // Every ASCII-aligned piece and separator carries zero `\r` bytes, so no seam can ever
    // merge and the naive sum above is already exact; only a non-ASCII join needs the fold.
    if !ascii {
        total_count = join_count_with_seams(parts, sep_ptr, sep_len, sep_count);
    }

    let (slice, content) = alloc_text_buffer(
        total_len as usize,
        total_count,
        inherited_flags(ascii, no_bidi),
    );
    let mut offset = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if content.is_null() {
            break; // `total_len == 0`: every piece and separator is empty, nothing to copy.
        }
        let part_bytes = byte_slice(part.data as *const u8, part.len);
        // SAFETY: `content` has room for `total_len` bytes, and `offset` never exceeds it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                part_bytes.as_ptr(),
                content.add(offset),
                part_bytes.len(),
            )
        };
        offset += part_bytes.len();
        if i + 1 < parts.len() {
            // SAFETY: same as above.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    sep_bytes.as_ptr(),
                    content.add(offset),
                    sep_bytes.len(),
                )
            };
            offset += sep_bytes.len();
        }
    }
    slice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::__gc_init;
    use crate::test_support::{GC_LOCK, header_of, slice_str, split_parts, text_of};

    #[test]
    fn grapheme_count_handles_ascii_and_multibyte() {
        let (ap, al) = text_of("hello");
        assert_eq!(__text_length(ap, al), 5);

        // "héllo" — the é is 2 bytes but 1 grapheme.
        let (mp, ml) = text_of("héllo");
        assert_eq!(ml, 6);
        assert_eq!(__text_length(mp, ml), 5);
    }

    #[test]
    fn grapheme_count_handles_emoji_clusters() {
        // Family emoji (ZWJ sequence) is many bytes / codepoints but one grapheme.
        let (fp, fl) = text_of("👨‍👩‍👧");
        assert!(fl > 4);
        assert_eq!(__text_length(fp, fl), 1);
    }

    #[test]
    fn text_length_null_and_empty_are_zero() {
        assert_eq!(__text_length(std::ptr::null(), 0), 0);
        let (xp, _) = text_of("x");
        assert_eq!(__text_length(xp, 0), 0);
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
    fn text_index_of_is_grapheme_based() {
        let (hp, hl) = text_of("héllo");
        let (sp, sl) = text_of("llo");
        // "llo" starts after "hé" — 2 graphemes in, even though "é" is 2 bytes.
        assert_eq!(__text_index_of(hp, hl, sp, sl), 2);
        let (zp, zl) = text_of("z");
        assert_eq!(__text_index_of(hp, hl, zp, zl), -1);
        // An empty needle is found at index 0.
        let (ep, el) = text_of("");
        assert_eq!(__text_index_of(hp, hl, ep, el), 0);
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
    fn text_graphemes_segments_clusters() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (gp, gl) = text_of("héllo");
        assert_eq!(
            split_parts(&__text_graphemes(gp, gl)),
            ["h", "é", "l", "l", "o"]
        );
        // An empty text has no graphemes.
        let (ep, el) = text_of("");
        assert!(split_parts(&__text_graphemes(ep, el)).is_empty());
        // A ZWJ family emoji is ONE grapheme, kept whole.
        let (fp, fl) = text_of("👨‍👩‍👧");
        assert_eq!(split_parts(&__text_graphemes(fp, fl)), ["👨‍👩‍👧"]);
    }

    #[test]
    fn text_at_indexes_graphemes_and_answers_empty_out_of_bounds() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // "a🌍b": graphemes a(0) 🌍(1) b(2) — 🌍 is 4 bytes but one grapheme.
        let (p, l) = text_of("a🌍b");
        assert_eq!(unsafe { slice_str(__text_at(p, l, 0)) }, "a");
        assert_eq!(unsafe { slice_str(__text_at(p, l, 1)) }, "🌍");
        assert_eq!(unsafe { slice_str(__text_at(p, l, 2)) }, "b");
        // Out of bounds — either side — is the empty answer, never a partial cluster.
        assert_eq!(__text_at(p, l, 3).len, 0);
        assert_eq!(__text_at(p, l, -1).len, 0);
        // A multi-codepoint cluster comes back whole.
        let (cp, cl) = text_of("e\u{0301}llo");
        assert_eq!(unsafe { slice_str(__text_at(cp, cl, 0)) }, "e\u{0301}");
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
    fn text_index_of_is_grapheme_correct_on_multibyte() {
        // "a🌍b": 🌍 is 4 bytes / 1 grapheme; "b" is at grapheme index 2, byte offset 5.
        let (hp, hl) = text_of("a🌍b");
        let (bp, bl) = text_of("b");
        assert_eq!(__text_index_of(hp, hl, bp, bl), 2);
        let (ep, el) = text_of("🌍");
        assert_eq!(__text_index_of(hp, hl, ep, el), 1);
        // 🌎 (U+1F30E) shares its first 3 UTF-8 bytes with 🌍 (U+1F30D) but differs in the
        // last — a byte-overlapping-but-different substring must NOT falsely match.
        let (fp, fl) = text_of("🌎");
        assert_eq!(__text_index_of(hp, hl, fp, fl), -1);
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
    fn replace_one_occurrence() {
        assert_eq!(
            replace_text("a-a-a", "a", "xx", 1.0),
            Ok("xx-a-a".to_string())
        );
    }

    #[test]
    fn replace_n_occurrences_left_to_right() {
        assert_eq!(
            replace_text("a-a-a", "a", "xx", 2.0),
            Ok("xx-xx-a".to_string())
        );
        assert_eq!(
            replace_text("a-a-a", "a", "xx", 3.0),
            Ok("xx-xx-xx".to_string())
        );
    }

    #[test]
    fn replace_count_truncates_toward_zero() {
        // 2.9 truncates to 2, not 3.
        assert_eq!(
            replace_text("a-a-a", "a", "xx", 2.9),
            Ok("xx-xx-a".to_string())
        );
    }

    #[test]
    fn replace_empty_from_fails() {
        assert_eq!(
            replace_text("abc", "", "x", 1.0),
            Err((
                codes::REPLACE_EMPTY_FROM,
                "replace: `from` must not be empty".to_string()
            ))
        );
    }

    #[test]
    fn replace_non_positive_count_fails() {
        assert_eq!(
            replace_text("abc", "b", "x", 0.0),
            Err((
                codes::REPLACE_INVALID_COUNT,
                "replace: count must be positive, got 0".to_string()
            ))
        );
        assert_eq!(
            replace_text("abc", "b", "x", -3.0),
            Err((
                codes::REPLACE_INVALID_COUNT,
                "replace: count must be positive, got -3".to_string()
            ))
        );
    }

    #[test]
    fn replace_count_over_occurrences_fails() {
        assert_eq!(
            replace_text("a-a-a", "a", "b", 5.0),
            Err((
                codes::REPLACE_INVALID_COUNT,
                "replace: count 5 exceeds 3 occurrences".to_string()
            ))
        );
    }

    /// `(count, ascii, valid_utf8, no_bidi)` at `ptr`, so a test states what it means
    /// rather than the bit pattern.
    fn header_bits(ptr: *const u8) -> (i64, bool, bool, bool) {
        let (count, flags) = header_of(ptr);
        (
            count,
            text_is_ascii(flags),
            text_is_valid_utf8(flags),
            text_no_bidi_controls(flags),
        )
    }

    #[test]
    fn alloc_text_headers_ascii_multibyte_cluster_and_empty_text() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();

        let ascii = alloc_text(b"hello");
        assert_eq!(header_bits(ascii.data as *const u8), (5, true, true, true));

        let multibyte = alloc_text("héllo".as_bytes());
        assert_eq!(
            header_bits(multibyte.data as *const u8),
            (5, false, true, true)
        );

        // A flag emoji (a regional-indicator pair) is one grapheme cluster over 8 bytes.
        let flag = alloc_text("🇮🇱".as_bytes());
        assert_eq!(header_bits(flag.data as *const u8), (1, false, true, true));

        // "e" + a combining acute is one grapheme cluster over 3 bytes.
        let combining = alloc_text("e\u{0301}".as_bytes());
        assert_eq!(
            header_bits(combining.data as *const u8),
            (1, false, true, true)
        );

        let empty = alloc_text(b"");
        assert!(empty.data.is_null());
        assert_eq!(header_bits(empty.data as *const u8), (0, true, true, true));
    }

    #[test]
    fn concat_sums_counts_and_ands_ascii_flags() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (ap, al) = text_of("ab");
        let (bp, bl) = text_of("cd");
        let both_ascii = __text_concat(ap, al, bp, bl);
        assert_eq!(
            header_bits(both_ascii.data as *const u8),
            (4, true, true, true)
        );
        assert_eq!(unsafe { slice_str(both_ascii) }, "abcd");

        let (ep, el) = text_of("é");
        let mixed = __text_concat(ap, al, ep, el);
        assert_eq!(header_bits(mixed.data as *const u8), (3, false, true, true));
    }

    #[test]
    fn concat_gate_skips_grapheme_work_when_both_boundary_chars_are_plain_letters() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        // Both operands are non-ASCII (each carries its own accented letter), but the
        // seam itself sits between two plain letters ('o' | 'w'), below
        // GRAPHEME_MERGE_FLOOR, so `seam_corrected` takes the O(1) gate, not the window.
        let (lp, ll) = text_of("héllo");
        let (rp, rl) = text_of("wörld");
        let joined = __text_concat(lp, ll, rp, rl);
        let (want, _) = crate::mem::text_header("héllowörld".as_bytes());
        assert_eq!(want, 10);
        assert_eq!(header_of(joined.data as *const u8).0, want);
    }

    #[test]
    fn concat_merges_a_combining_mark_spanning_the_seam() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (lp, ll) = text_of("e");
        let (rp, rl) = text_of("\u{0301}");
        let joined = __text_concat(lp, ll, rp, rl);
        let (want, _) = crate::mem::text_header("e\u{0301}".as_bytes());
        assert_eq!(want, 1);
        assert_eq!(header_of(joined.data as *const u8).0, want);
    }

    #[test]
    fn concat_merges_cr_lf_spanning_the_seam_and_drops_the_ascii_bit() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (lp, ll) = text_of("\r");
        let (rp, rl) = text_of("\n");
        let joined = __text_concat(lp, ll, rp, rl);
        let (want, want_flags) = crate::mem::text_header("\r\n".as_bytes());
        assert_eq!(want, 1);
        let (count, ascii, _, _) = header_bits(joined.data as *const u8);
        assert_eq!(count, want);
        assert_eq!(ascii, text_is_ascii(want_flags));
    }

    #[test]
    fn concat_across_a_cr_lf_seam_slices_the_merged_grapheme_correctly() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (lp, ll) = text_of("ab\r");
        let (rp, rl) = text_of("\ncd");
        let joined = __text_concat(lp, ll, rp, rl);
        // "ab\r\ncd": graphemes a(0) b(1) [\r\n](2) c(3) d(4) — 5 graphemes, not 6.
        assert_eq!(header_of(joined.data as *const u8).0, 5);
        let third = __text_slice(joined.data as *const u8, joined.len, 2, 3);
        assert_eq!(unsafe { slice_str(third) }, "\r\n");
        let at2 = __text_at(joined.data as *const u8, joined.len, 2);
        assert_eq!(unsafe { slice_str(at2) }, "\r\n");
    }

    #[test]
    fn concat_pairs_regional_indicators_spanning_the_seam() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (lp, ll) = text_of("🇦");
        let (rp, rl) = text_of("🇧🇨");
        let joined = __text_concat(lp, ll, rp, rl);
        assert_eq!(header_of(joined.data as *const u8).0, 2);
    }

    #[test]
    fn join_merges_a_grapheme_spanning_a_part_and_the_separator() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (ep, el) = text_of("e");
        let (mp, ml) = text_of("\u{0301}");
        let parts = [
            QlSlice {
                data: ep as *const c_void,
                len: el,
            },
            QlSlice {
                data: mp as *const c_void,
                len: ml,
            },
        ];
        let (sep_p, sep_l) = text_of("");
        let joined = __text_join(
            parts.as_ptr() as *const c_void,
            parts.len() as i64,
            sep_p,
            sep_l,
        );
        assert_eq!(header_of(joined.data as *const u8).0, 1);
    }

    #[test]
    fn slice_header_is_the_span_length_with_no_grapheme_walk() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (p, l) = text_of("héllo");
        let sliced = __text_slice(p, l, 1, 4);
        let (count, ascii, valid_utf8, _) = header_bits(sliced.data as *const u8);
        assert_eq!((count, ascii, valid_utf8), (3, false, true));

        let (ap, al) = text_of("hello");
        assert_eq!(
            header_bits(__text_slice(ap, al, 1, 4).data as *const u8),
            (3, true, true, true)
        );
    }

    #[test]
    fn split_pieces_and_replace_carry_correct_headers() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (hp, hl) = text_of("héllo,world");
        let (comma_p, comma_l) = text_of(",");
        let parts = __text_split(hp, hl, comma_p, comma_l);
        let elems =
            unsafe { std::slice::from_raw_parts(parts.data as *const QlSlice, parts.len as usize) };
        assert_eq!(
            header_bits(elems[0].data as *const u8),
            (5, false, true, true)
        );
        assert_eq!(
            header_bits(elems[1].data as *const u8),
            (5, true, true, true)
        );

        let (rp, rl) = text_of("a-a-a");
        let (from_p, from_l) = text_of("a");
        let (to_p, to_l) = text_of("é");
        let replaced_all = __text_replace_all(rp, rl, from_p, from_l, to_p, to_l, std::ptr::null());
        assert_eq!(
            header_bits(replaced_all.data as *const u8),
            (5, false, true, true)
        );

        let (x_p, x_l) = text_of("x");
        let replaced = __text_replace(rp, rl, from_p, from_l, x_p, x_l, 1.0, std::ptr::null());
        assert_eq!(
            header_bits(replaced.data as *const u8),
            (5, true, true, true)
        );
    }

    #[test]
    fn join_sums_counts_and_ascii_flags() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let (ap, al) = text_of("ab");
        let (ep, el) = text_of("é");
        let parts = [
            QlSlice {
                data: ap as *const c_void,
                len: al,
            },
            QlSlice {
                data: ep as *const c_void,
                len: el,
            },
        ];
        let (sep_p, sep_l) = text_of(",");
        let joined = __text_join(
            parts.as_ptr() as *const c_void,
            parts.len() as i64,
            sep_p,
            sep_l,
        );
        // "ab" (2) + "," (1) + "é" (1) = 4 graphemes; not ASCII-aligned, since "é" isn't.
        assert_eq!(
            header_bits(joined.data as *const u8),
            (4, false, true, true)
        );
        assert_eq!(unsafe { slice_str(joined) }, "ab,é");

        let empty: [QlSlice; 0] = [];
        let empty_join = __text_join(empty.as_ptr() as *const c_void, 0, sep_p, sep_l);
        assert!(empty_join.data.is_null());
    }

    #[test]
    fn index_of_from_finds_at_or_after_the_given_grapheme() {
        let (hp, hl) = text_of("a-a-a");
        let (sp, sl) = text_of("a");
        assert_eq!(__text_index_of_from(hp, hl, sp, sl, 0), 0);
        assert_eq!(__text_index_of_from(hp, hl, sp, sl, 1), 2);
        assert_eq!(__text_index_of_from(hp, hl, sp, sl, 3), 4);
        // Past the end, or negative, both clamp to a valid search origin rather than fail.
        assert_eq!(__text_index_of_from(hp, hl, sp, sl, 100), -1);
        assert_eq!(__text_index_of_from(hp, hl, sp, sl, -5), 0);

        // Non-ASCII haystack: "héllo", searching for "l" from grapheme 3 (the second "l").
        let (np, nl) = text_of("héllo");
        let (lp, ll) = text_of("l");
        assert_eq!(__text_index_of_from(np, nl, lp, ll, 0), 2);
        assert_eq!(__text_index_of_from(np, nl, lp, ll, 3), 3);
        assert_eq!(__text_index_of_from(np, nl, lp, ll, 4), -1);
    }
}
