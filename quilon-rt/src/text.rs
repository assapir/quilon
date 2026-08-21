// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Text intrinsics — each backs a named, chainable `Text` method (`length`,
//! `cmp`, `trim`, case mapping, `contains`/`indexOf`, `replace`, `slice`,
//! `split`). All are UTF-8 correct and grapheme-based where an index/length is
//! user-visible (matching `Text.length`). A `Text` argument arrives as
//! `(ptr, len)`; a `Text` / `[]Text` result is returned as a GC-allocated
//! `QlSlice` so it outlives this call and is collected like any heap value. See
//! `CodeGenerator::get_intrinsic` for the matching prototypes.

use crate::mem::{__alloc, QlSlice, alloc_text, format_num};
use crate::report::{QlSite, fail_at};
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

/// Abort on an invalid `replace`/`replaceAll`/`repeat` request (empty `from`, non-positive
/// `count`, or a `count` exceeding the occurrences present): report `msg` at `site` — the
/// framed diagnostic an assertion failure also produces — and exit 101. Never returns. The
/// detection lives in the runtime (not codegen) because the `count > occurrences` case needs
/// the occurrence count.
fn text_misuse(site: *const QlSite, msg: &str) -> ! {
    fail_at(site, msg, 101)
}

/// Repeat the text `count` times, back to back. Backs `Text.repeat(count)`; `count` 0
/// yields the empty text. Fails loudly (aborts the process, exit 101) on a `count` that is
/// negative, fractional, or NaN — the checker rejects those at compile time when they are
/// literal, and this is the runtime backstop for a computed one.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __text_repeat(
    ptr: *const u8,
    len: i64,
    count: f64,
    site: *const QlSite,
) -> QlSlice {
    if !count.is_finite() || count < 0.0 || count.fract() != 0.0 {
        text_misuse(site, "repeat: `count` must be a whole number of 0 or more");
    }
    alloc_text(text_str(ptr, len).repeat(count as usize).as_bytes())
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
    site: *const QlSite,
) -> QlSlice {
    let hay = text_str(hptr, hlen);
    let from = text_str(fptr, flen);
    if from.is_empty() {
        text_misuse(site, "replace: `from` must not be empty");
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
    site: *const QlSite,
) -> QlSlice {
    let hay = text_str(hptr, hlen);
    let from = text_str(fptr, flen);
    // Fail loudly on invalid input — no clamp, no silent no-op (see `text_misuse`).
    if from.is_empty() {
        text_misuse(site, "replace: `from` must not be empty");
    }
    if count <= 0 {
        text_misuse(
            site,
            &format!("replace: count must be positive, got {count}"),
        );
    }
    // Non-overlapping, left→right occurrences — exactly what `replacen` consumes.
    let occurrences = hay.matches(&*from).count() as i64;
    if count > occurrences {
        text_misuse(
            site,
            &format!("replace: count {count} exceeds {occurrences} occurrences"),
        );
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
            unsafe {
                slice_str(__text_replace_n(
                    hp,
                    hl,
                    fp,
                    fl,
                    tp,
                    tl,
                    1,
                    std::ptr::null(),
                ))
            },
            "xx-a-a"
        );
        // count = 2 -> first two.
        assert_eq!(
            unsafe {
                slice_str(__text_replace_n(
                    hp,
                    hl,
                    fp,
                    fl,
                    tp,
                    tl,
                    2,
                    std::ptr::null(),
                ))
            },
            "xx-xx-a"
        );
        // count == exact number of occurrences -> all three.
        assert_eq!(
            unsafe {
                slice_str(__text_replace_n(
                    hp,
                    hl,
                    fp,
                    fl,
                    tp,
                    tl,
                    3,
                    std::ptr::null(),
                ))
            },
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
            unsafe { slice_str(__text_replace_all(hp, hl, fp, fl, tp, tl, std::ptr::null())) },
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
        let (hp, hl) = text_of("a,,b");
        let (sp, sl) = text_of(",");
        assert_eq!(split_parts(&__text_split(hp, hl, sp, sl)), ["a", "", "b"]);
        // Empty haystack -> a single empty piece.
        let (ep, el) = text_of("");
        assert_eq!(split_parts(&__text_split(ep, el, sp, sl)), [""]);
        // Empty separator -> graphemes.
        let (gp, gl) = text_of("héllo");
        let (esp, esl) = text_of("");
        assert_eq!(
            split_parts(&__text_split(gp, gl, esp, esl)),
            ["h", "é", "l", "l", "o"]
        );
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
            unsafe { slice_str(__text_replace_all(hp, hl, fp, fl, tp, tl, std::ptr::null())) },
            "aébéc"
        );
        // replace count = 1: only the first.
        assert_eq!(
            unsafe {
                slice_str(__text_replace_n(
                    hp,
                    hl,
                    fp,
                    fl,
                    tp,
                    tl,
                    1,
                    std::ptr::null(),
                ))
            },
            "aéb🌍c"
        );
    }
}
