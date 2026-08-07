// Built-in array methods: `map` / `filter` / `reduce` / `each` / `find` / `at`.
// These are compiler-provided members on arrays, called with method syntax
// (`arr.map(f)`) and chainable. The higher-order forms take a lambda the compiler
// INLINES (Quilon has no first-class closures). `find`/`at` return a `Result`
// (`Ok(elem)` / `NotOk`). These tests drive the full pipeline (lex -> parse ->
// typecheck -> codegen -> JIT) and assert the real exit code.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / target init isn't thread-safe; cargo runs tests in parallel.
static JIT_LOCK: Mutex<()> = Mutex::new(());

fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .expect("type checking failed");

    let code = jit::run_program(&program).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

/// A source that should FAIL the front-end (lex/parse/typecheck).
fn assert_rejected(src: &str) {
    let tokens = match Lexer::tokenize(src) {
        Ok(t) => t,
        Err(_) => return,
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected the front-end to reject:\n{src}"
    );
}

// ---- map ----------------------------------------------------------------

/// `map` produces a new array; sum the doubled elements via reduce. [1,2,3] -> [2,4,6] -> 12.
#[test]
fn map_doubles_elements() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  a.map(x => x * 2).reduce(0, (acc, x) => acc + x)\n>",
        12,
    );
}

/// `map` preserves length and order: index into the mapped array.
#[test]
fn map_preserves_order_and_length() {
    assert_exit(
        "^ = () -> Num => <\n  m = [1, 2, 3, 4].map(x => x + 10)\n  m.size * 100 + m[0]\n>",
        // size 4 -> 400, m[0] = 11 -> 411
        411,
    );
}

// ---- filter -------------------------------------------------------------

/// `filter` keeps only matching elements, in order.
#[test]
fn filter_keeps_matches() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2, 3, 4, 5, 6]\n  big = a.filter(x => x > 3)\n  big.size * 10 + big[0]\n>",
        // [4,5,6]: size 3 -> 30, first 4 -> 34
        34,
    );
}

/// `filter` that keeps nothing yields a size-0 array.
#[test]
fn filter_can_keep_nothing() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  none = a.filter(x => x > 99)\n  none.size\n>",
        0,
    );
}

// ---- reduce -------------------------------------------------------------

/// `reduce` folds left from the initial accumulator.
#[test]
fn reduce_sums() {
    assert_exit(
        "^ = () -> Num => <\n  [10, 20, 30].reduce(5, (acc, x) => acc + x)\n>",
        65,
    );
}

// ---- chaining -----------------------------------------------------------

/// map -> filter -> reduce chains end-to-end.
#[test]
fn chain_map_filter_reduce() {
    assert_exit(
        "^ = () -> Num => <\n  [1, 2, 3, 4, 5, 6]\n    .map(x => x * 2)\n    .filter(x => x > 4)\n    .reduce(0, (acc, x) => acc + x)\n>",
        // [2,4,6,8,10,12] -> keep >4 -> [6,8,10,12] -> 36
        36,
    );
}

// ---- each ---------------------------------------------------------------

/// `each` returns the receiver array (decision 19), so it chains: `.each(f).size`.
#[test]
fn each_returns_receiver_and_chains() {
    assert_exit(
        "^ = () -> Num => <\n  a = [7, 8, 9]\n  a.each(x => x).size\n>",
        3,
    );
}

// ---- find ---------------------------------------------------------------

/// `find` returns `Ok(elem)` of the first match.
#[test]
fn find_ok_first_match() {
    assert_exit(
        "^ = () -> Num => <\n  r = [1, 2, 3, 4].find(x => x > 2) ?\n    | Ok(v) => v\n    | NotOk(_) => 0\n  r\n>",
        3,
    );
}

/// `find` with no match returns `NotOk`.
#[test]
fn find_notok_when_absent() {
    assert_exit(
        "^ = () -> Num => <\n  r = [1, 2, 3].find(x => x > 99) ?\n    | Ok(v) => v\n    | NotOk(_) => 42\n  r\n>",
        42,
    );
}

// ---- at -----------------------------------------------------------------

/// `at` returns `Ok(elem)` for an in-bounds index.
#[test]
fn at_ok_in_bounds() {
    assert_exit(
        "^ = () -> Num => <\n  r = [10, 20, 30].at(1) ?\n    | Ok(v) => v\n    | NotOk(_) => 0\n  r\n>",
        20,
    );
}

/// `at` returns `NotOk` for an out-of-bounds index (both above and below).
#[test]
fn at_notok_out_of_bounds() {
    assert_exit(
        "^ = () -> Num => <\n  hi = [10, 20, 30].at(9) ?\n    | Ok(v) => v\n    | NotOk(_) => 1\n  lo = [10, 20, 30].at(0) ?\n    | Ok(v) => v\n    | NotOk(_) => 0\n  hi + lo\n>",
        // at(9) -> NotOk -> 1 ; at(0) -> Ok(10) -> 10 ; total 11
        11,
    );
}

// ---- []Text (oracle integration) ---------------------------------------

/// `map`/`reduce` over `[]Text` works — proving the element type flows from the
/// type oracle (not a hardcoded f64). Concatenate then count graphemes.
#[test]
fn map_reduce_over_text_array() {
    assert_exit(
        "^ = () -> Num => <\n  ws = [\"foo\", \"bar\"]\n  ws.map(w => w + \"!\").reduce(\"\", (acc, w) => acc + w).length\n>",
        // "foo!" + "bar!" = "foo!bar!" -> 8 graphemes
        8,
    );
}

/// `find` over `[]Text` yields `Ok(text)` — the Result payload is the Text element.
#[test]
fn find_over_text_array() {
    assert_exit(
        "^ = () -> Num => <\n  ws = [\"a\", \"bbbb\", \"cc\"]\n  ws.find(w => w.length > 1) ?\n    | Ok(w) => w.length\n    | NotOk(_) => 0\n>",
        // first with length > 1 is "bbbb" -> 4
        4,
    );
}

// ---- overload coexistence -----------------------------------------------

/// A user function named `map` on a non-array type coexists with the reserved
/// array `map`: `a.map(...)` resolves to the built-in, `map(n)` to the user def.
#[test]
fn array_method_reserved_over_user_definition() {
    assert_exit(
        "map = (n :: Num) -> Num => n + 100\n^ = () -> Num => <\n  a = [1, 2, 3]\n  s = a.map(x => x * 2).reduce(0, (acc, x) => acc + x)\n  s + map(5)\n>",
        // [2,4,6] -> 12 ; map(5) -> 105 ; total 117
        117,
    );
}

// ---- rejection ----------------------------------------------------------

/// A `filter` predicate that doesn't return `Bool` is rejected.
#[test]
fn filter_predicate_must_be_bool() {
    assert_rejected("^ = () -> Num => <\n  [1, 2, 3].filter(x => x + 1).size\n>");
}

/// `at` requires a `Num` index.
#[test]
fn at_index_must_be_num() {
    assert_rejected("^ = () -> Num => <\n  [1, 2, 3].at(\"oops\")\n>");
}

/// A lambda passed somewhere other than a built-in array method (here, to `print`)
/// is rejected — Quilon has no first-class function values.
#[test]
fn bare_lambda_is_not_a_value() {
    assert_rejected("<< core.io\n^ = () -> Num => <\n  print(x => x)\n  0\n>");
}
