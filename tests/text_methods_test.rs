//! End-to-end tests for the built-in `Text` methods (split/trim/replace/contains/
//! indexOf/slice/toUpper/toLower). Drives the full pipeline (lex -> parse -> typecheck
//! -> codegen -> JIT) and asserts the program's real exit code, mirroring `run_test.rs`.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// LLVM JIT / native-target init isn't thread-safe; serialize execution.
static JIT_LOCK: Mutex<()> = Mutex::new(());

// Unique suffix for temp files across parallel subprocess crash tests.
static CRASH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Run `src` via `quilon run` in a SUBPROCESS and assert it aborts with exit code 101
/// and prints `expect_stderr` to stderr. Used for the fail-loud runtime paths (an invalid
/// `replace` argument) — the abort exits the process, so these must NOT run in-process
/// (that would kill the test runner). Needs only the JIT (no C toolchain).
fn assert_run_aborts(src: &str, expect_stderr: &str) {
    let seq = CRASH_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("quilon_replace_abort_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("prog.ql");
    std::fs::write(&file, src).expect("write temp program");
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .output()
        .expect("run quilon run");
    let code = out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        code, 101,
        "expected abort exit 101 for source:\n{src}\ngot {code}; stderr: {stderr}"
    );
    assert!(
        stderr.contains(expect_stderr),
        "stderr {stderr:?} missing {expect_stderr:?} for source:\n{src}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Compile and run `src`, asserting the entry point yields `expected`.
fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code = jit::run_program(&program, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

/// Assert `src` fails to type-check (a negative test).
fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    assert!(
        TypeChecker::new().check_program(&program).is_err(),
        "expected a type error for source:\n{}",
        src
    );
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
    assert_exit("^ = () -> Num => \"  héllo  \".trim().size", 6); // 6 bytes ("é" is 2)
}

#[test]
fn trim_chains() {
    // trim then toUpper, verified by content equality.
    assert_exit(
        "^ = () -> Num => \"  hi  \".trim().toUpper() == \"HI\" ? 1 : 0",
        1,
    );
}

// ---- replaceAll / replace(count) ------------------------------------------

#[test]
fn replace_all_replaces_every_occurrence() {
    // "a-a-a".replaceAll("a","xx") -> "xx-xx-xx" (size 8).
    assert_exit(
        "^ = () -> Num => \"a-a-a\".replaceAll(\"a\", \"xx\").size",
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
        "^ = () -> Num => \"a-a-a\".replace(\"a\", \"xx\", 2.9).size",
        7,
    );
}

// Compile-time rejections (literal-determinable).
#[test]
fn replace_literal_count_zero_or_negative_is_a_compile_error() {
    assert_type_error("^ = () -> Num => \"a-a-a\".replace(\"a\", \"b\", 0).size");
    assert_type_error("^ = () -> Num => \"a-a-a\".replace(\"a\", \"b\", -2).size");
}

#[test]
fn replace_literal_empty_from_is_a_compile_error() {
    assert_type_error("^ = () -> Num => \"abc\".replace(\"\", \"x\", 1).size");
}

#[test]
fn replace_all_literal_empty_from_is_a_compile_error() {
    assert_type_error("^ = () -> Num => \"abc\".replaceAll(\"\", \"x\").size");
}

#[test]
fn replace_literal_count_over_occurrences_is_a_compile_error() {
    // "a-a-a" has 3 "a"; asking for 5 is a compile error (all operands literal).
    assert_type_error("^ = () -> Num => \"a-a-a\".replace(\"a\", \"b\", 5).size");
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
        "^ = () -> Num => \"héllo\".indexOf(\"llo\") ? | Ok(i) => i | NotOk(_) => 99",
        2,
    );
}

#[test]
fn index_of_notok_when_absent() {
    assert_exit(
        "^ = () -> Num => \"Hello\".indexOf(\"z\") ? | Ok(_) => 50 | NotOk(_) => 7",
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
    assert_exit("^ = () -> Num => \"héllo\".slice(0, 2).size", 3);
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
    // A user `contains` on Num coexists; the Text receiver still resolves the built-in.
    assert_exit(
        "contains = (n :: Num) -> Bool => n > 0\n^ = () -> Num => <\n  a = \"Hello\".contains(\"ell\") ? 1 : 0\n  b = contains(5) ? 1 : 0\n  a * 10 + b\n>",
        11,
    );
}

// ---- Unicode correctness (end-to-end) ------------------------------------

#[test]
fn split_on_multibyte_separator() {
    // "a🌍b🌍c".split("🌍") -> ["a","b","c"], size 3 (4-byte emoji separator).
    assert_exit("^ = () -> Num => \"a🌍b🌍c\".split(\"🌍\").size", 3);
}

#[test]
fn split_empty_separator_keeps_multi_codepoint_cluster_whole() {
    // A ZWJ family emoji is ONE grapheme -> empty-sep split yields a single element.
    assert_exit("^ = () -> Num => \"👨‍👩‍👧\".split(\"\").size", 1);
}

#[test]
fn index_of_is_grapheme_index_across_emoji() {
    // "a🌍b": 🌍 is 4 bytes / 1 grapheme, so "b" is at grapheme index 2 (not byte 5).
    assert_exit(
        "^ = () -> Num => \"a🌍b\".indexOf(\"b\") ? | Ok(i) => i | NotOk(_) => 99",
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
        "^ = () -> Num => \"héllo\".slice(1, 3) == \"él\" ? 1 : 0",
        1,
    );
    // The sliced text is valid: "él" is 3 bytes (é=2, l=1).
    assert_exit("^ = () -> Num => \"héllo\".slice(1, 3).size", 3);
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
        "^ = () -> Num => \"\u{00A0}héllo\u{00A0}\".trim() == \"héllo\" ? 1 : 0",
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
        "^ = () -> Num => \"  hi  \".trimStart() == \"hi  \" ? 1 : 0",
        1,
    );
    assert_exit(
        "^ = () -> Num => \"  hi  \".trimEnd() == \"  hi\" ? 1 : 0",
        1,
    );
}

#[test]
fn trim_start_and_end_are_unicode_whitespace_aware() {
    // NBSP (U+00A0) on both ends; trimStart removes the leading one only, trimEnd the
    // trailing one only.
    assert_exit(
        "^ = () -> Num => \"\u{00A0}héllo\u{00A0}\".trimStart() == \"héllo\u{00A0}\" ? 1 : 0",
        1,
    );
    assert_exit(
        "^ = () -> Num => \"\u{00A0}héllo\u{00A0}\".trimEnd() == \"\u{00A0}héllo\" ? 1 : 0",
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
    assert_type_error("^ = () -> Num => \"Hello\".slice(\"a\", 2).size");
}

#[test]
fn replace_count_must_be_a_num() {
    // The 3rd arg is a Num count — a non-Num is a type error.
    assert_type_error("^ = () -> Num => \"a-a-a\".replace(\"a\", \"b\", true).size");
}
