// `+` on arrays builds a NEW array (never mutating an operand), in three exact-type
// forms, all dispatched by the operands' exact types:
//   concat:  []T + []T -> []T   ([1,2] + [3,4] -> [1,2,3,4])
//   append:  []T + T   -> []T   ([1,2] + 5     -> [1,2,5])
//   prepend: T + []T   -> []T   (0 + [1,2]     -> [0,1,2])
//
// These forms are mutually exclusive (`[]T` can never equal its element `T`), so there
// is never ambiguity — including the nested case `[][]Num + []Num`, which binds as an
// APPEND (the `[]Num` is a single element). These tests drive the full pipeline
// (lex -> parse -> typecheck -> codegen -> JIT) and cover `[]Num`, `[]Text`, nested
// arrays, non-mutation of the operands, and the element-type-mismatch type errors.

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

    let code = jit::run_program(&program, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected a type error for source:\n{}",
        src
    );
}

// ---------------------------------------------------------------------------
// concat: []T + []T -> []T
// ---------------------------------------------------------------------------

/// `[1,2] + [3,4]` -> `[1,2,3,4]`: elements of the left then the right, in order.
#[test]
fn concat_num_arrays() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2]\n  b = [3, 4]\n  c = a + b\n  c.size * 100 + c[0] * 10 + c[3]\n>",
        // size 4 -> 400, c[0]=1 -> 10, c[3]=4 => 414
        414,
    );
}

/// Concatenation copies ELEMENTS (element-repr-correct), so a `[]Text` concat round-trips
/// its texts — not just `[]Num`.
#[test]
fn concat_text_arrays() {
    assert_exit(
        "^ = () -> Num => <\n  a = [\"ab\", \"c\"]\n  b = [\"de\"]\n  c = a + b\n  c.size * 100 + c[0].size * 10 + c[2].size\n>",
        // size 3 -> 300, "ab".size 2 -> 20, "de".size 2 -> 2 => 322
        322,
    );
}

/// `+` is left-associative, so `a + b + c` concatenates all three in order.
#[test]
fn concat_chains_left_to_right() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1]\n  b = [2]\n  c = [3]\n  d = a + b + c\n  d.size * 100 + d[0] * 10 + d[2]\n>",
        // [1,2,3]: size 3 -> 300, d[0]=1 -> 10, d[2]=3 => 313
        313,
    );
}

/// Concatenating with an empty array is an identity (same elements, same order).
#[test]
fn concat_with_empty_array() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  b = a + []\n  b.size * 10 + b[2]\n>",
        // size 3 -> 30, b[2]=3 => 33
        33,
    );
}

// ---------------------------------------------------------------------------
// append: []T + T -> []T   /   prepend: T + []T -> []T
// ---------------------------------------------------------------------------

/// `[1,2] + 5` -> `[1,2,5]`: append a single element to the end.
#[test]
fn append_num_element() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2]\n  b = a + 5\n  b.size * 100 + b[2]\n>",
        // size 3 -> 300, b[2]=5 => 305
        305,
    );
}

/// `0 + [1,2]` -> `[0,1,2]`: prepend a single element to the front.
#[test]
fn prepend_num_element() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2]\n  b = 0 + a\n  b.size * 100 + b[0] * 10 + b[2]\n>",
        // size 3 -> 300, b[0]=0 -> 0, b[2]=2 => 302
        302,
    );
}

/// Append/prepend are element-repr-correct too: `["Ada"] + "Lovelace"` (append) and
/// `"Hi" + ["there"]` (prepend) both round-trip their `Text` elements.
#[test]
fn append_and_prepend_text_element() {
    assert_exit(
        "^ = () -> Num => <\n  named = [\"Ada\"] + \"Lovelace\"\n  greet = \"Hi\" + [\"there\"]\n  named.size * 1000 + named[1].size * 100 + greet.size * 10 + greet[0].size\n>",
        // named.size 2 -> 2000, "Lovelace".size 8 -> 800, greet.size 2 -> 20, "Hi".size 2 -> 2 => 2822
        2822,
    );
}

/// A prepend then append mixed with a concat, left-to-right: `0 + a + 9` is
/// `((0 + a) + 9)` = `[0,1,2,9]`.
#[test]
fn mixed_prepend_append_chain() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2]\n  b = 0 + a + 9\n  b.size * 100 + b[0] * 10 + b[3]\n>",
        // [0,1,2,9]: size 4 -> 400, b[0]=0 -> 0, b[3]=9 => 409
        409,
    );
}

// ---------------------------------------------------------------------------
// nested arrays — the disambiguation edge
// ---------------------------------------------------------------------------

/// `[][]Num + []Num` is an APPEND: the `[]Num` equals the element type, so it is a SINGLE
/// element (a new row), yielding `[][]Num` — NOT a concat.
#[test]
fn nested_array_plus_inner_is_append() {
    assert_exit(
        "^ = () -> Num => <\n  rows = [[1, 2], [3, 4]]\n  rows2 = rows + [5, 6]\n  rows2.size * 100 + rows2[2][0] * 10 + rows2[2][1]\n>",
        // rows2 = [[1,2],[3,4],[5,6]]: size 3 -> 300, rows2[2][0]=5 -> 50, rows2[2][1]=6 => 356
        356,
    );
}

/// `[][]Num + [][]Num` is a CONCAT: both sides are the same array type, so the rows are
/// spliced (not nested), yielding a longer `[][]Num`.
#[test]
fn nested_array_plus_same_is_concat() {
    assert_exit(
        "^ = () -> Num => <\n  rows = [[1, 2], [3, 4]]\n  grid = rows + rows\n  grid.size * 100 + grid[2][0] * 10 + grid[3][1]\n>",
        // grid = [[1,2],[3,4],[1,2],[3,4]]: size 4 -> 400, grid[2][0]=1 -> 10, grid[3][1]=4 => 414
        414,
    );
}

// ---------------------------------------------------------------------------
// non-mutation: `+` always yields a NEW array; operands are unchanged
// ---------------------------------------------------------------------------

/// Neither operand is mutated by concat/append/prepend — the originals keep their size
/// and elements after being used in every `+` form.
#[test]
fn operands_are_not_mutated() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2, 3]\n  b = [4, 5]\n  c = a + b\n  d = a + 9\n  e = 0 + a\n  a.size * 1000 + b.size * 100 + a[0] * 10 + a[2]\n>",
        // a.size still 3 -> 3000, b.size still 2 -> 200, a[0]=1 -> 10, a[2]=3 => 3213
        3213,
    );
}

// ---------------------------------------------------------------------------
// element-type mismatches are type errors
// ---------------------------------------------------------------------------

/// `[]Num + []Text` is a type error (mismatched element types — neither concat, append,
/// nor prepend applies).
#[test]
fn concat_mismatched_element_types_is_type_error() {
    assert_type_error("^ = () -> Num => <\n  a = [1, 2]\n  b = [\"x\"]\n  c = a + b\n  0\n>");
}

/// `[]Num + Text` is a type error: the appended element must match the array's element
/// type.
#[test]
fn append_wrong_element_type_is_type_error() {
    assert_type_error("^ = () -> Num => <\n  a = [1, 2]\n  c = a + \"x\"\n  0\n>");
}

/// `Text + []Num` is a type error: the prepended element must match the array's element
/// type.
#[test]
fn prepend_wrong_element_type_is_type_error() {
    assert_type_error("^ = () -> Num => <\n  a = [1, 2]\n  c = \"x\" + a\n  0\n>");
}
