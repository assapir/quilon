// Ranges: the infix `<-` operator builds an inclusive `[]Num`.
//   `1 <- 4` -> [1, 2, 3, 4]      (inclusive)
//   `4 <- 1` -> [4, 3, 2, 1]      (descends when the left end is larger)
// It is array sugar — no distinct Range type — so the result composes with
// `.size`, indexing, and `for`. These tests drive the full pipeline (lex ->
// parse -> typecheck -> codegen -> JIT) and assert the real exit code.

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

/// `(1 <- 4).size == 4` — an inclusive range has `|hi - lo| + 1` elements.
/// (`.size` needs a named receiver in 0.9, so bind the range first.)
#[test]
fn range_size_is_inclusive() {
    assert_exit("^ = () -> Num => <\n  r = 1 <- 4\n  r.size\n>", 4);
}

/// A single-point range `5 <- 5` is `[5]` — size 1.
#[test]
fn range_single_point_has_size_one() {
    assert_exit("^ = () -> Num => <\n  r = 5 <- 5\n  r.size\n>", 1);
}

/// Ascending `1 <- 4` is `[1, 2, 3, 4]`: summing the four endpoints by index
/// gives 1 + 2 + 3 + 4 = 10. (Index-summed, not loop-accumulated, so the test
/// is independent of mutable-accumulator behavior.)
#[test]
fn ascending_range_values_in_order() {
    assert_exit(
        "^ = () -> Num => <\n  r = 1 <- 4\n  r[0] + r[1] + r[2] + r[3]\n>",
        10,
    );
}

/// Descending `4 <- 1` is `[4, 3, 2, 1]`: the first element is the LARGER end.
/// Encode the order as 1000*r[0] + 100*r[1] + 10*r[2] + r[3] = 4321.
#[test]
fn descending_range_is_reversed() {
    assert_exit(
        "^ = () -> Num => <\n  r = 4 <- 1\n  1000*r[0] + 100*r[1] + 10*r[2] + r[3]\n>",
        4321,
    );
}

/// Range ends can be dynamic (not just literals): `a <- b` with bound `a`/`b`
/// still materializes correctly, and chooses direction at runtime.
#[test]
fn range_with_dynamic_ends() {
    assert_exit(
        "^ = () -> Num => <\n  a = 2\n  b = 5\n  r = a <- b\n  r.size + r[0] + r[3]\n>",
        // [2,3,4,5]: size 4 + first 2 + last 5 = 11
        11,
    );
}

/// A range is just a `[]Num`, so it drives a `for` loop like any array.
#[test]
fn range_drives_for_loop() {
    // for over [1,2,3] yields Num 0 (loop result), then return the size to prove
    // the range materialized.
    assert_exit(
        "^ = () -> Num => <\n  r = 1 <- 3\n  for n <- r => n\n  r.size\n>",
        3,
    );
}

/// CRITICAL coexistence: the new infix `<-` must NOT break the `for` header's
/// own `<-`. `for n <- [...]` must still parse, type-check, and run end-to-end
/// exactly as before. (Parse-shape coexistence — that the header parses as a
/// `ForLoop`, not a `Range` — is asserted separately in the parser unit tests.)
#[test]
fn for_loop_over_literal_array_still_runs() {
    assert_exit(
        "^ = () -> Num => <\n  xs = [10, 20, 30]\n  for n <- xs => n\n  xs.size\n>",
        3,
    );
}
