// Closures (M3): execution tests for lexical capture, with the capture mode chosen by
// the binding operator — `=` captures by value (a frozen snapshot), `:=` captures by
// reference (a shared, mutable cell whose writes escape the closure). Each test drives
// the full pipeline (lex -> parse -> typecheck -> codegen -> JIT) and asserts the real
// exit code, the same backbone as run_test.rs.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / native-target init is not thread-safe; serialize across parallel tests.
static JIT_LOCK: Mutex<()> = Mutex::new(());

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

fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    assert!(
        TypeChecker::new().check_program(&program).is_err(),
        "expected a type error for source:\n{}",
        src
    );
}

// --- `:=` capture is by reference: writes from inside the closure escape and accumulate
//     across separate calls (it shares one cell with the enclosing frame). ---
#[test]
fn mutable_capture_counter_accumulates() {
    // `count` is `:=`, captured by reference. Each `bump(...)` writes the shared cell, so
    // the effect of the first call is visible to the second: 10 then +20 -> 30.
    assert_exit(
        "^ = () -> Num => <\n  count := 0\n  bump = n => <\n    count := count + n\n    count\n  >\n  bump(10)\n  bump(20)\n  count\n>",
        30,
    );
}

// --- A `:=` capture sees writes the OUTER frame makes after the closure is created —
//     direct evidence the cell is shared, not copied. ---
#[test]
fn mutable_capture_sees_later_outer_write() {
    assert_exit(
        "^ = () -> Num => <\n  n := 1\n  readN = () => n\n  n := 100\n  readN()\n>",
        100,
    );
}

// --- `=` capture is by value: the closure carries a frozen snapshot of the captured
//     binding, usable repeatedly and independent of anything else. ---
#[test]
fn value_capture_is_frozen_snapshot() {
    // `base` is `=` (immutable), captured by value. add(10)=17, add(20)=27 -> 44.
    assert_exit(
        "^ = () -> Num => <\n  base = 7\n  add = x => x + base\n  add(10) + add(20)\n>",
        44,
    );
}

// --- A closure capturing nothing is just a function value, callable normally. ---
#[test]
fn closure_with_no_captures() {
    assert_exit(
        "^ = () -> Num => <\n  twice = x => x + x\n  twice(21)\n>",
        42,
    );
}

// --- Two closures capturing the SAME `:=` cell share it: a write through one is seen by
//     the other (the box is one shared mutable cell). ---
#[test]
fn two_closures_share_one_mutable_cell() {
    assert_exit(
        "^ = () -> Num => <\n  total := 0\n  add = n => <\n    total := total + n\n    total\n  >\n  reset = () => <\n    total := 0\n    total\n  >\n  add(5)\n  add(37)\n  reset()\n  add(42)\n>",
        42,
    );
}

// --- A closure may capture by value AND by reference at once. ---
#[test]
fn mixed_value_and_reference_capture() {
    // `step` (=) is frozen-copied; `acc` (:=) is the shared cell. acc starts 0,
    // bump adds step (3) each call: 3, then 6.
    assert_exit(
        "^ = () -> Num => <\n  step = 3\n  acc := 0\n  bump = () => <\n    acc := acc + step\n    acc\n  >\n  bump()\n  bump()\n>",
        6,
    );
}

// --- A closure parameter still shadows an outer binding of the same name (the param is
//     not a capture). ---
#[test]
fn parameter_shadows_outer_binding() {
    assert_exit(
        "^ = () -> Num => <\n  x = 99\n  f = x => x + 1\n  f(41)\n>",
        42,
    );
}

// --- Capture does not relax mutability: an `=`-captured name is still immutable, so the
//     checker rejects writing it (there is no `:=` reassign of an `=` binding). ---
#[test]
fn value_capture_stays_immutable() {
    assert_type_error(
        "^ = () -> Num => <\n  k = 1\n  f = () => <\n    k := 2\n    k\n  >\n  f()\n>",
    );
}

// --- A nested function that captures NOTHING is a plain local function and may recurse
//     (a closure value cannot refer to itself before it exists, so non-capturing nested
//     functions are emitted as ordinary functions, preserving recursion). ---
#[test]
fn non_capturing_nested_function_recurses() {
    assert_exit(
        "^ = () -> Num => <\n  fact = n -> Num => n == 0 ? 1 : n * fact(n - 1)\n  fact(5)\n>",
        120,
    );
}

// --- A closure may capture another closure value and call it (higher-order use within a
//     frame): `g` captures the capturing closure `f` and invokes it. ---
#[test]
fn closure_captures_and_calls_another_closure() {
    assert_exit(
        "^ = () -> Num => <\n  base = 40\n  f = x => x + base\n  g = () => f(1)\n  g() + 1\n>",
        42,
    );
}

// --- A `:=` cell mutated through TWO levels of closure nesting still shares one cell:
//     the inner closure's writes are visible after both calls (10 = 5 + 5). ---
#[test]
fn mutable_capture_shared_across_two_nesting_levels() {
    assert_exit(
        "^ = () -> Num => <\n  total := 0\n  mid = () => <\n    inner = () => <\n      total := total + 5\n      total\n    >\n    inner()\n    inner()\n  >\n  mid()\n  total\n>",
        10,
    );
}

// --- A closure nested two levels deep can read an `=` value from the outermost frame,
//     captured transitively through the middle closure (frozen by value). ---
#[test]
fn value_capture_threads_through_nested_closure() {
    assert_exit(
        "^ = () -> Num => <\n  a = 42\n  mid = z => <\n    inner = w => a + w\n    inner(z)\n  >\n  mid(0)\n>",
        42,
    );
}
