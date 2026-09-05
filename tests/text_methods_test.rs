//! End-to-end tests for the built-in `Text` methods (split/trim/replace/contains/
//! indexOf/slice/toUpper/toLower). Drives the full pipeline (lex -> parse -> typecheck
//! -> codegen -> JIT) and asserts the program's real exit code, mirroring `run_test.rs`.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

// Unique suffix for temp files across parallel subprocess crash tests.

mod common;
use common::{assert_exit, assert_type_error};

static CRASH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Run `src` via `quilon run` in a SUBPROCESS and assert it aborts with exit code 101
/// and prints `expect_stderr` to stderr. Used for the fail-loud runtime paths (an invalid
/// `replace` argument) — the abort exits the process, so these must NOT run in-process
/// (that would kill the test runner). Needs only the JIT (no C toolchain).
fn assert_run_aborts(src: &str, expect_stderr: &str) {
    let (code, stderr) = run_and_capture(src);
    assert_eq!(
        code, 101,
        "expected abort exit 101 for source:\n{src}\ngot {code}; stderr: {stderr}"
    );
    assert!(
        stderr.contains(expect_stderr),
        "stderr {stderr:?} missing {expect_stderr:?} for source:\n{src}"
    );
}

/// Run `src` as a subprocess (never the in-process JIT — these programs call `__exit`,
/// which would take the test runner with them) and return `(exit code, stderr)`.
fn run_and_capture(src: &str) -> (i32, String) {
    let seq = CRASH_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("quilon_replace_abort_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("prog.qn");
    std::fs::write(&file, src).expect("write temp program");
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .output()
        .expect("run quilon run");
    let captured = (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    let _ = std::fs::remove_dir_all(&dir);
    captured
}

// ---- split ----------------------------------------------------------------

#[test]
fn split_basic_size() {
    // "a,b,c".split(",") -> ["a","b","c"], size 3.
    assert_exit(
        "^ = () -> Num => <\n  p = \"a,b,c\".split(\",\")\n  p.size\n>",
        3,
    );
}

#[test]
fn split_multichar_separator_and_element_is_text() {
    // Pieces are real Text values (usable by the Text API and comparisons).
    assert_exit(
        "^ = () -> Num => <\n  p = \"Hello, World\".split(\", \")\n  p.at(0) ?\n    | Ok(w)    => w == \"Hello\" ? p.size : 0\n    | NotOk(_) => 0\n>",
        2,
    );
}

#[test]
fn split_preserves_empty_pieces() {
    // "a,,b".split(",") -> ["a","","b"], size 3 (empties preserved).
    assert_exit(
        "^ = () -> Num => <\n  p = \"a,,b\".split(\",\")\n  mid = p.at(1) ?\n    | Ok(m)    => m.size\n    | NotOk(_) => 99\n  p.size * 10 + mid\n>",
        30,
    );
}

#[test]
fn split_empty_haystack_is_single_empty() {
    // "".split(",") -> [""], size 1.
    assert_exit("^ = () -> Num => <\n  \"\".split(\",\").size\n>", 1);
}

#[test]
fn split_empty_separator_is_graphemes() {
    // "héllo".split("") -> ["h","é","l","l","o"], size 5 (grapheme-based).
    assert_exit("^ = () -> Num => <\n  \"héllo\".split(\"\").size\n>", 5);
}

// ---- trim -----------------------------------------------------------------

#[test]
fn trim_strips_surrounding_whitespace() {
    assert_exit("^ = () -> Num => < \"  héllo  \".trim().size >", 6); // 6 bytes ("é" is 2)
}

#[test]
fn trim_chains() {
    // trim then toUpper, verified by content equality.
    assert_exit(
        "^ = () -> Num => < \"  hi  \".trim().toUpper() == \"HI\" ? 1 : 0 >",
        1,
    );
}

// ---- replaceAll / replace(count) ------------------------------------------

#[test]
fn replace_all_replaces_every_occurrence() {
    // "a-a-a".replaceAll("a","xx") -> "xx-xx-xx" (size 8).
    assert_exit(
        "^ = () -> Num => < \"a-a-a\".replaceAll(\"a\", \"xx\").size >",
        8,
    );
}

#[test]
fn replace_exact_count_left_to_right() {
    // count 1 -> "xx-a-a" (6), 2 -> "xx-xx-a" (7), 3 (== occurrences) -> "xx-xx-xx" (8).
    assert_exit(
        "^ = () -> Num => <\n  a = \"a-a-a\".replace(\"a\", \"xx\", 1).size\n  b = \"a-a-a\".replace(\"a\", \"xx\", 2).size\n  c = \"a-a-a\".replace(\"a\", \"xx\", 3).size\n  a * 100 + b * 10 + c\n>",
        678,
    );
}

#[test]
fn replace_count_truncates_toward_zero() {
    // 2.9 truncates to 2 -> "xx-xx-a" (size 7).
    assert_exit(
        "^ = () -> Num => < \"a-a-a\".replace(\"a\", \"xx\", 2.9).size >",
        7,
    );
}

// Compile-time rejections (literal-determinable).
#[test]
fn replace_literal_count_zero_or_negative_is_a_compile_error() {
    assert_type_error("^ = () -> Num => < \"a-a-a\".replace(\"a\", \"b\", 0).size >");
    assert_type_error("^ = () -> Num => < \"a-a-a\".replace(\"a\", \"b\", -2).size >");
}

#[test]
fn replace_literal_empty_from_is_a_compile_error() {
    assert_type_error("^ = () -> Num => < \"abc\".replace(\"\", \"x\", 1).size >");
}

#[test]
fn replace_all_literal_empty_from_is_a_compile_error() {
    assert_type_error("^ = () -> Num => < \"abc\".replaceAll(\"\", \"x\").size >");
}

#[test]
fn replace_literal_count_over_occurrences_is_a_compile_error() {
    // "a-a-a" has 3 "a"; asking for 5 is a compile error (all operands literal).
    assert_type_error("^ = () -> Num => < \"a-a-a\".replace(\"a\", \"b\", 5).size >");
}

// Runtime fail-loud (non-literal, so not caught at compile time) — abort, no silent no-op.
#[test]
fn replace_runtime_count_zero_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  n = 3 - 3\n  \"a-a-a\".replace(\"a\", \"b\", n).size\n>",
        "count must be positive",
    );
}

#[test]
fn replace_runtime_count_over_occurrences_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  n = 2 + 3\n  \"a-a-a\".replace(\"a\", \"b\", n).size\n>",
        "exceeds",
    );
}

#[test]
fn replace_runtime_empty_from_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  f = \"\"\n  \"abc\".replace(f, \"x\", 1).size\n>",
        "must not be empty",
    );
}

#[test]
fn replace_all_runtime_empty_from_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  f = \"\"\n  \"abc\".replaceAll(f, \"x\").size\n>",
        "must not be empty",
    );
}

// ---- repeat ---------------------------------------------------------------

#[test]
fn repeat_concatenates_count_copies() {
    // "ab".repeat(3) -> "ababab" (size 6); repeat(1) is the text itself.
    assert_exit(
        "^ = () -> Num => <\n  a = \"ab\".repeat(3).size\n  b = \"ab\".repeat(1).size\n  a * 10 + b\n>",
        62,
    );
}

#[test]
fn repeat_zero_is_the_empty_text() {
    assert_exit("^ = () -> Num => < \"ab\".repeat(0).size >", 0);
}

#[test]
fn repeat_is_grapheme_safe() {
    // A 2-grapheme, multi-byte text repeated 3 times: 6 graphemes, bytes intact.
    assert_exit("^ = () -> Num => < \"é😀\".repeat(3).length >", 6);
}

#[test]
fn repeat_composes_with_other_text_methods() {
    assert_exit(
        "^ = () -> Num => < \"-\".repeat(4).contains(\"----\") ? 1 : 0 >",
        1,
    );
}

// Compile-time rejections (literal-determinable), mirroring `replace`'s contract.
#[test]
fn repeat_literal_negative_or_fractional_count_is_a_compile_error() {
    assert_type_error("^ = () -> Num => < \"ab\".repeat(-1).size >");
    assert_type_error("^ = () -> Num => < \"ab\".repeat(2.5).size >");
}

// Runtime fail-loud for a computed count — abort, never a silent clamp.
/// A violated contract reports WHERE the call is, in the same frame a failing assertion
/// uses — so `replace`'s misuse messages name the offending call rather than leaving the
/// reader to find which of several `replace`s it was.
#[test]
fn a_replace_misuse_reports_its_own_location() {
    let (code, stderr) = run_and_capture(
        "^ = () -> Num => <\n  n = 2 + 3\n  \"a-a-a\".replace(\"a\", \"b\", n).size\n>",
    );
    assert_eq!(code, 101);
    assert!(
        stderr.contains(":3:3:\nreplace: count 5 exceeds 3 occurrences"),
        "the report must locate the call, got: {stderr}"
    );
    assert!(
        stderr.contains("3 |   \"a-a-a\".replace(\"a\", \"b\", n).size"),
        "the report must show the source line, got: {stderr}"
    );
    // The carets cover the CALL — `"a-a-a".replace("a", "b", n)` — not the trailing
    // `.size` the result feeds into.
    let carets = stderr
        .lines()
        .filter_map(|line| line.rsplit_once('|').map(|(_, rest)| rest.trim()))
        .find(|rest| rest.starts_with('^'))
        .unwrap_or_default();
    assert_eq!(
        carets.len(),
        "\"a-a-a\".replace(\"a\", \"b\", n)".len(),
        "the caret run must be exactly as wide as the call, got: {stderr}"
    );
}

/// Same for `repeat`'s count contract, and for a call that is not the first thing on its
/// line: the caret run starts at the call, not at the line.
#[test]
fn a_repeat_misuse_reports_its_own_location() {
    let (code, stderr) =
        run_and_capture("^ = () -> Num => <\n  n = 1 - 4\n  x = \"ab\".repeat(n).size\n  x\n>");
    assert_eq!(code, 101);
    assert!(
        stderr.contains(":3:7:\nrepeat: `count` must be a whole number of 0 or more"),
        "the report must locate the call at its column, got: {stderr}"
    );
}

#[test]
fn repeat_runtime_negative_count_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  n = 1 - 4\n  \"ab\".repeat(n).size\n>",
        "whole number",
    );
}

#[test]
fn repeat_runtime_fractional_count_aborts() {
    assert_run_aborts(
        "^ = () -> Num => <\n  n = 7 / 2\n  \"ab\".repeat(n).size\n>",
        "whole number",
    );
}

// ---- contains -------------------------------------------------------------

#[test]
fn contains_hit_and_miss() {
    assert_exit(
        "^ = () -> Num => <\n  h = \"Hello\".contains(\"ell\") ? 1 : 0\n  m = \"Hello\".contains(\"zzz\") ? 1 : 0\n  h * 10 + m\n>",
        10,
    );
}

// ---- indexOf --------------------------------------------------------------

#[test]
fn index_of_ok_is_grapheme_index() {
    // "héllo".indexOf("llo") -> Ok(2) (grapheme index, not byte offset 3).
    assert_exit(
        "^ = () -> Num => < \"héllo\".indexOf(\"llo\") ? | Ok(i) => i | NotOk(_) => 99 >",
        2,
    );
}

#[test]
fn index_of_notok_when_absent() {
    assert_exit(
        "^ = () -> Num => < \"Hello\".indexOf(\"z\") ? | Ok(_) => 50 | NotOk(_) => 7 >",
        7,
    );
}

// ---- slice ----------------------------------------------------------------

#[test]
fn slice_basic_and_clamp() {
    // [1,4) -> "ell" (3); clamp both ends -> whole (5); end<=start -> empty (0).
    assert_exit(
        "^ = () -> Num => <\n  a = \"Hello\".slice(1, 4).size\n  b = \"Hello\".slice(-5, 100).size\n  c = \"Hello\".slice(3, 1).size\n  a * 100 + b * 10 + c\n>",
        350,
    );
}

#[test]
fn slice_is_grapheme_based() {
    // "héllo".slice(0, 2) -> "hé" = 3 bytes (grapheme indices, not byte indices).
    assert_exit("^ = () -> Num => < \"héllo\".slice(0, 2).size >", 3);
}

// ---- case mapping ---------------------------------------------------------

#[test]
fn to_upper_and_to_lower() {
    assert_exit(
        "^ = () -> Num => <\n  u = \"héllo\".toUpper() == \"HÉLLO\" ? 1 : 0\n  l = \"HÉLLO\".toLower() == \"héllo\" ? 1 : 0\n  u * 10 + l\n>",
        11,
    );
}

// ---- []Text is a plain generic array (composes with all array ops + `+`) --
// `split` returns `[]Text` = the generic `[]T` with `T = Text` (like `[]Num`), NOT a
// special-cased type. These prove it behaves like any other array.

#[test]
fn split_result_indexes_and_sizes_like_any_array() {
    assert_exit(
        "^ = () -> Num => <\n  xs = \"a,b,c\".split(\",\")\n  xs.size * 10 + (xs[1] == \"b\" ? 1 : 0)\n>",
        31,
    );
}

#[test]
fn split_result_indexing_yields_a_text_element() {
    // Indexing returns a real `Text` (full Text API works on the element).
    assert_exit(
        "^ = () -> Num => <\n  xs = \"aa,bbb,c\".split(\",\")\n  xs[1].size\n>",
        3,
    );
}

#[test]
fn split_result_supports_map_and_reduce() {
    // map over `[]Text` -> `[]Num` (element type Text from the oracle), then fold.
    assert_exit(
        "^ = () -> Num => <\n  xs = \"aa,b,ccc\".split(\",\")\n  xs.map(w => w.size).reduce(0, (a, x) => a + x)\n>",
        6,
    );
}

#[test]
fn split_result_supports_filter() {
    assert_exit(
        "^ = () -> Num => <\n  xs = \"a,bb,c,dd\".split(\",\")\n  xs.filter(w => w.size == 2).size\n>",
        2,
    );
}

#[test]
fn split_result_supports_find_and_at() {
    assert_exit(
        "^ = () -> Num => <\n  xs = \"a,bb,ccc\".split(\",\")\n  f = xs.find(w => w.size == 3) ? | Ok(v) => v.size | NotOk(_) => 0\n  a = xs.at(1) ? | Ok(v) => v.size | NotOk(_) => 0\n  oob = xs.at(9) ? | Ok(_) => 1 | NotOk(_) => 5\n  f * 100 + a * 10 + oob\n>",
        325,
    );
}

#[test]
fn split_result_supports_each_and_chains() {
    // `.each` returns the receiver array, so it chains (here back into `.size`).
    assert_exit(
        "^ = () -> Num => <\n  xs = \"a,b,c\".split(\",\")\n  ys = xs.each(w => w)\n  ys.size\n>",
        3,
    );
}

#[test]
fn empty_separator_grapheme_split_composes_with_map() {
    // A grapheme split is also a plain `[]Text`: map its one-grapheme pieces to sizes.
    assert_exit(
        "^ = () -> Num => <\n  xs = \"héllo\".split(\"\")\n  xs.map(g => g.size).reduce(0, (a, x) => a + x)\n>",
        6, // "héllo" = 6 bytes total across 5 single-grapheme pieces
    );
}

#[test]
fn split_results_concatenate_via_array_plus() {
    // `[]Text + []Text -> []Text`: split(...) + split(...) concatenates like any array.
    assert_exit(
        "^ = () -> Num => <\n  c = \"a,b\".split(\",\") + \"c,d,e\".split(\",\")\n  c.size * 10 + (c[3] == \"d\" ? 1 : 0)\n>",
        51,
    );
}

// ---- reservation / overloading -------------------------------------------

#[test]
fn text_method_wins_over_user_overload_on_text_receiver() {
    // A user `trim` on Num coexists; the Text receiver still resolves the built-in.
    // (`contains` itself is a reserved name — the matcher — so a Text method name that is
    // not one stands in.)
    assert_exit(
        "trim = (n :: Num) -> Bool => < n > 0 >\n^ = () -> Num => <\n  a = \"  Hello \".trim().size == 5 ? 1 : 0\n  b = trim(5) ? 1 : 0\n  a * 10 + b\n>",
        11,
    );
}

// ---- Unicode correctness (end-to-end) ------------------------------------

#[test]
fn split_on_multibyte_separator() {
    // "a🌍b🌍c".split("🌍") -> ["a","b","c"], size 3 (4-byte emoji separator).
    assert_exit("^ = () -> Num => < \"a🌍b🌍c\".split(\"🌍\").size >", 3);
}

#[test]
fn split_empty_separator_keeps_multi_codepoint_cluster_whole() {
    // A ZWJ family emoji is ONE grapheme -> empty-sep split yields a single element.
    assert_exit("^ = () -> Num => < \"👨‍👩‍👧\".split(\"\").size >", 1);
}

#[test]
fn index_of_is_grapheme_index_across_emoji() {
    // "a🌍b": 🌍 is 4 bytes / 1 grapheme, so "b" is at grapheme index 2 (not byte 5).
    assert_exit(
        "^ = () -> Num => < \"a🌍b\".indexOf(\"b\") ? | Ok(i) => i | NotOk(_) => 99 >",
        2,
    );
}

#[test]
fn contains_matches_multibyte_and_rejects_byte_overlap() {
    // 🌎 shares its first 3 UTF-8 bytes with 🌍 but is a different grapheme: no false hit.
    assert_exit(
        "^ = () -> Num => <\n  hit  = \"a🌍b\".contains(\"🌍\") ? 1 : 0\n  miss = \"a🌍b\".contains(\"🌎\") ? 1 : 0\n  hit * 10 + miss\n>",
        10,
    );
}

#[test]
fn slice_does_not_split_a_multibyte_codepoint() {
    // "héllo".slice(1, 3) -> "él" (graphemes 1..3), never a half-encoded "é".
    assert_exit(
        "^ = () -> Num => < \"héllo\".slice(1, 3) == \"él\" ? 1 : 0 >",
        1,
    );
    // The sliced text is valid: "él" is 3 bytes (é=2, l=1).
    assert_exit("^ = () -> Num => < \"héllo\".slice(1, 3).size >", 3);
}

#[test]
fn case_mapping_is_unicode_aware() {
    // Non-ASCII round-trips, and the 1->N mapping "ß".toUpper() == "SS".
    assert_exit(
        "^ = () -> Num => <\n  up   = \"é\".toUpper() == \"É\" ? 1 : 0\n  lo   = \"Ä\".toLower() == \"ä\" ? 1 : 0\n  sharp = \"ß\".toUpper() == \"SS\" ? 1 : 0\n  up * 100 + lo * 10 + sharp\n>",
        111,
    );
}

#[test]
fn trim_strips_unicode_whitespace() {
    // Leading/trailing NBSP (U+00A0) is Unicode whitespace and must be trimmed.
    assert_exit(
        "^ = () -> Num => < \"\u{00A0}héllo\u{00A0}\".trim() == \"héllo\" ? 1 : 0 >",
        1,
    );
}

// ---- trimStart / trimEnd --------------------------------------------------

#[test]
fn trim_start_and_end_strip_one_side_only() {
    // "  hi  ".trimStart() -> "hi  " (4); .trimEnd() -> "  hi" (4).
    assert_exit(
        "^ = () -> Num => <\n  s = \"  hi  \".trimStart().size\n  e = \"  hi  \".trimEnd().size\n  s * 10 + e\n>",
        44,
    );
    // Content check: only the intended side is stripped.
    assert_exit(
        "^ = () -> Num => < \"  hi  \".trimStart() == \"hi  \" ? 1 : 0 >",
        1,
    );
    assert_exit(
        "^ = () -> Num => < \"  hi  \".trimEnd() == \"  hi\" ? 1 : 0 >",
        1,
    );
}

#[test]
fn trim_start_and_end_are_unicode_whitespace_aware() {
    // NBSP (U+00A0) on both ends; trimStart removes the leading one only, trimEnd the
    // trailing one only.
    assert_exit(
        "^ = () -> Num => < \"\u{00A0}héllo\u{00A0}\".trimStart() == \"héllo\u{00A0}\" ? 1 : 0 >",
        1,
    );
    assert_exit(
        "^ = () -> Num => < \"\u{00A0}héllo\u{00A0}\".trimEnd() == \"\u{00A0}héllo\" ? 1 : 0 >",
        1,
    );
}

#[test]
fn replace_with_multibyte_from_and_to() {
    // Replace a 4-byte emoji separator with a 2-byte "é": replaceAll vs first (count 1).
    assert_exit(
        "^ = () -> Num => <\n  all   = \"a🌍b🌍c\".replaceAll(\"🌍\", \"é\") == \"aébéc\" ? 1 : 0\n  first = \"a🌍b🌍c\".replace(\"🌍\", \"é\", 1) == \"aéb🌍c\" ? 1 : 0\n  all * 10 + first\n>",
        11,
    );
}

#[test]
fn slice_rejects_non_num_indices() {
    assert_type_error("^ = () -> Num => < \"Hello\".slice(\"a\", 2).size >");
}

#[test]
fn replace_count_must_be_a_num() {
    // The 3rd arg is a Num count — a non-Num is a type error.
    assert_type_error("^ = () -> Num => < \"a-a-a\".replace(\"a\", \"b\", true).size >");
}

// ---- at / graphemes (the grapheme-access primitives) ----------------------

#[test]
fn at_reads_one_grapheme_and_is_notok_out_of_bounds() {
    // "a🌍b": at(1) is the whole 4-byte emoji; at(3) and at(-1) are NotOk.
    assert_exit(
        "^ = () -> Num => <\n  g = \"a🌍b\".at(1) ?\n    | Ok(t)    => t == \"🌍\" ? 1 : 0\n    | NotOk(_) => 9\n  over = \"a🌍b\".at(3) ?\n    | Ok(_)    => 9\n    | NotOk(_) => 1\n  under = \"a🌍b\".at(0 - 1) ?\n    | Ok(_)    => 9\n    | NotOk(_) => 1\n  g * 100 + over * 10 + under\n>",
        111,
    );
}

#[test]
fn at_keeps_a_multi_codepoint_cluster_whole() {
    // NFD "é" (e + combining acute) is one grapheme of 3 bytes — never split.
    assert_exit(
        "^ = () -> Num => <\n  \"e\u{0301}llo\".at(0) ?\n    | Ok(g)    => g.size\n    | NotOk(_) => 0\n>",
        3,
    );
}

#[test]
fn graphemes_yields_the_cluster_sequence() {
    // 5 graphemes; the empty text has none; a ZWJ family emoji is ONE.
    assert_exit("^ = () -> Num => < \"héllo\".graphemes().size >", 5);
    assert_exit("^ = () -> Num => < \"\".graphemes().size >", 0);
    assert_exit("^ = () -> Num => < \"👨‍👩‍👧\".graphemes().size >", 1);
}

#[test]
fn graphemes_composes_with_array_methods() {
    // The []Text of graphemes goes through filter/map like any array.
    assert_exit(
        "^ = () -> Num => < \"a,b,c\".graphemes().filter(g => g == \",\").size >",
        2,
    );
}

#[test]
fn at_takes_exactly_one_num_index() {
    assert_type_error(
        "^ = () -> Num => <\n  \"abc\".at(\"x\") ?\n    | Ok(_) => 1\n    | NotOk(_) => 0\n>",
    );
}

// ---- bidi text corpus (Hebrew/Arabic, LOGICAL order) -----------------------
//
// `Text` is stored and processed in LOGICAL order (docs/types/text.md): every index,
// length, search, and concatenation addresses the order the text was TYPED in, never the
// order a bidi-aware display would draw it in. These are not new operations — the same
// `length`/`graphemes`/`at`/`slice`/`indexOf`/`+`/`split` above — run over Hebrew, Arabic,
// and mixed-direction literals to confirm right-to-left script makes no difference to any
// of them.

#[test]
fn hebrew_literal_length_and_size() {
    // "שלום" (Hebrew "hello"): 4 graphemes, 2 UTF-8 bytes each -> 8 bytes.
    assert_exit(
        "^ = () -> Num => <\n  t = \"שלום\"\n  t.length * 100 + t.size\n>",
        408,
    );
}

#[test]
fn arabic_literal_length_and_size() {
    // "مرحبا" (Arabic "hello"): 5 graphemes, 2 UTF-8 bytes each -> 10 bytes.
    assert_exit(
        "^ = () -> Num => <\n  t = \"مرحبا\"\n  t.length * 100 + t.size\n>",
        510,
    );
}

#[test]
fn hebrew_graphemes_and_at_are_logical_order() {
    // The FIRST grapheme is the FIRST-TYPED letter (`ש`) — the one a right-to-left reader
    // encounters last — not whatever a bidi-aware display would draw in that position.
    assert_exit(
        "^ = () -> Num => <\n  t = \"שלום\"\n  count = t.graphemes().size\n  first = t.at(0) ?\n    | Ok(g)    => g == \"ש\" ? 1 : 0\n    | NotOk(_) => 0\n  count * 10 + first\n>",
        41,
    );
}

#[test]
fn hebrew_slice_is_logical_order() {
    // slice(0, 2) takes the first TWO TYPED letters ("של"), not the last two.
    assert_exit(
        "^ = () -> Num => < \"שלום\".slice(0, 2) == \"של\" ? 1 : 0 >",
        1,
    );
}

#[test]
fn mixed_direction_index_of_is_logical_order() {
    // "שלום world": the Hebrew word is typed FIRST, so "world" starts at grapheme index 5
    // (4 Hebrew letters + 1 space) — its typed position, not its visual one.
    assert_exit(
        "^ = () -> Num => < \"שלום world\".indexOf(\"world\") ? | Ok(i) => i | NotOk(_) => 99 >",
        5,
    );
}

#[test]
fn arabic_and_digits_index_of_is_logical_order() {
    // "مرحبا 123": the digits are typed AFTER the Arabic word, so `indexOf` finds them at
    // grapheme index 6 (5 Arabic letters + 1 space) regardless of Arabic being RTL and
    // digits being LTR.
    assert_exit(
        "^ = () -> Num => < \"مرحبا 123\".indexOf(\"123\") ? | Ok(i) => i | NotOk(_) => 99 >",
        6,
    );
}

#[test]
fn plus_concatenates_scripts_in_logical_order() {
    // `+` builds the typed order, Hebrew then Arabic, with nothing reordered or inserted.
    assert_exit(
        "^ = () -> Num => <\n  s = \"שלום\" + \" \" + \"مرحبا\"\n  matches = s == \"שלום مرحبا\" ? 1 : 0\n  matches * 10000 + s.length * 100 + s.size\n>",
        11019,
    );
}

#[test]
fn split_a_mixed_direction_sentence_keeps_logical_piece_order() {
    // A mixed sentence with punctuation: the three comma-separated pieces come back in the
    // order they were TYPED — Hebrew, then Arabic, then the Latin word.
    assert_exit(
        "^ = () -> Num => <\n  parts = \"שלום, مرحبا, world\".split(\", \")\n  a = parts[0] == \"שלום\" ? 1 : 0\n  b = parts[1] == \"مرحبا\" ? 1 : 0\n  c = parts[2] == \"world\" ? 1 : 0\n  parts.size * 1000 + a * 100 + b * 10 + c\n>",
        3111,
    );
}

#[test]
fn rtl_literal_with_trailing_punctuation_is_logical_order() {
    // "שלום!": the exclamation mark is typed (and stored) LAST, regardless of how a
    // bidi-aware display might position it relative to the Hebrew letters.
    assert_exit(
        "^ = () -> Num => <\n  t = \"שלום!\"\n  length_ok = t.length == 5 ? 1 : 0\n  last = t.at(4) ?\n    | Ok(g)    => g == \"!\" ? 1 : 0\n    | NotOk(_) => 0\n  length_ok * 10 + last\n>",
        11,
    );
}
