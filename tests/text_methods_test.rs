//! End-to-end tests for the built-in `Text` methods (split/trim/replace/contains/
//! indexOf/slice/toUpper/toLower). Drives the full pipeline (lex -> parse -> typecheck
//! -> codegen -> JIT) and asserts the program's real exit code, mirroring `run_test.rs`.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / native-target init isn't thread-safe; serialize execution.
static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Compile and run `src`, asserting the entry point yields `expected`.
fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code = jit::run_program(&program).expect("execution failed");
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

// ---- replace --------------------------------------------------------------

#[test]
fn replace_all_vs_first() {
    // all -> "xx-xx-xx" (8), first -> "xx-a-a" (6).
    assert_exit(
        "^ = () -> Num => <\n  a = \"a-a-a\".replace(\"a\", \"xx\", true).size\n  f = \"a-a-a\".replace(\"a\", \"xx\", false).size\n  a * 10 + f\n>",
        86,
    );
}

#[test]
fn replace_empty_from_is_noop() {
    assert_exit(
        "^ = () -> Num => \"abc\".replace(\"\", \"x\", true) == \"abc\" ? 1 : 0",
        1,
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

// ---- reservation / overloading -------------------------------------------

#[test]
fn text_method_wins_over_user_overload_on_text_receiver() {
    // A user `contains` on Num coexists; the Text receiver still resolves the built-in.
    assert_exit(
        "contains = (n :: Num) -> Bool => n > 0\n^ = () -> Num => <\n  a = \"Hello\".contains(\"ell\") ? 1 : 0\n  b = contains(5) ? 1 : 0\n  a * 10 + b\n>",
        11,
    );
}

#[test]
fn slice_rejects_non_num_indices() {
    assert_type_error("^ = () -> Num => \"Hello\".slice(\"a\", 2).size");
}

#[test]
fn replace_rejects_non_bool_flag() {
    assert_type_error("^ = () -> Num => \"a\".replace(\"a\", \"b\", 1).size");
}
