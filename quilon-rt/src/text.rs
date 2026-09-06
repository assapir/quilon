// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Text intrinsics — the PRIMITIVE floor under the built-in `Text` methods:
//! segmentation (`length`, `graphemes`, `at`), comparison (`cmp`), the
//! whitespace walks (`trimStart`/`trimEnd`), case mapping, substring search
//! (`indexOf`), grapheme-boundary `slice`, and the two byte-linear walks
//! (`split`, `replaceAll`). The remaining composable methods
//! (`trim`/`contains`/`replace`/`repeat`) are written in Quilon over these
//! (`corelib/text.qn`), so they are deliberately NOT here.
//! All are UTF-8 correct and grapheme-based where an index/length is
//! user-visible (matching `Text.length`). A `Text` argument arrives as
//! `(ptr, len)`; a `Text` / `[]Text` result is returned as a GC-allocated
//! `QlSlice` so it outlives this call and is collected like any heap value. See
//! `CodeGenerator::get_intrinsic` for the matching prototypes.

use crate::mem::{QlSlice, alloc_slots, alloc_text, format_num};
use crate::report::{ASSERTION_EXIT_CODE, QlSite, codes, fail_at};
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

/// View `len` bytes at `ptr` as a slice (empty for null/non-positive `len`). Shared with
/// [`crate::report`], which reads the `Text` fields of a call site the same way.
pub(crate) fn byte_slice<'a>(ptr: *const u8, len: i64) -> &'a [u8] {
    if ptr.is_null() || len <= 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    }
}

/// Decode `len` bytes at `ptr` as UTF-8 (lossily on invalid UTF-8, which a
/// well-formed Quilon `Text` never is). Shared by all the Text-method intrinsics.
///
/// # Safety contract (upheld by the compiler)
/// `ptr` is null or points to at least `len` readable bytes.
pub(crate) fn text_str<'a>(ptr: *const u8, len: i64) -> std::borrow::Cow<'a, str> {
    String::from_utf8_lossy(byte_slice(ptr, len))
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
/// `Text.at(index)`, without segmenting past the asked-for grapheme.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_at(ptr: *const u8, len: i64, index: i64) -> QlSlice {
    if index < 0 {
        return QlSlice::empty();
    }
    let s = text_str(ptr, len);
    match s.graphemes(true).nth(index as usize) {
        Some(grapheme) => alloc_text(grapheme.as_bytes()),
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
            codes::REPLACE_ALL_EMPTY_FROM,
            "replaceAll: `from` must not be empty",
            ASSERTION_EXIT_CODE,
        );
    }
    let hay = text_str(hptr, hlen);
    let to = text_str(tptr, tlen);
    alloc_text(hay.replace(&*from, &to).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::__gc_init;
    use crate::test_support::{GC_LOCK, slice_str, split_parts, text_of};

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
}
