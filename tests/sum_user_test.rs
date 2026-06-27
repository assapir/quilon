//! End-to-end tests for user-defined sum types (the `/` separator):
//! declaration, construction, and exhaustive pattern matching that extracts
//! payloads. Drives the full pipeline (lex -> parse -> typecheck -> codegen ->
//! JIT) and asserts the entry point's real exit code, plus negative
//! type-checking cases (non-exhaustive match, bad payload type, duplicate
//! variant names). Result is exercised here too, as a *normal* predefined sum
//! type, to prove the general mechanism subsumes the old special case.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / native-target init isn't thread-safe; cargo runs tests in parallel.
static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Compile and run `src`, asserting the entry point yields `expected`.
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

/// Assert `src` fails type checking (a negative test).
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

#[test]
fn nullary_enum_matched_exhaustively() {
    // A nullary enum `Color`, constructed and matched on every variant.
    // Green is the second variant (tag 1); the match maps it to exit code 1.
    assert_exit(
        "Color = Red / Green / Blue\n\
         ^ = () -> Num => <\n\
           c = Green\n\
           c ?\n\
             | Red => 0\n\
             | Green => 1\n\
             | Blue => 2\n\
         >",
        1,
    );
}

#[test]
fn payload_sum_constructed_and_matched() {
    // `Shape` with payload variants; match extracts the payloads. A Rect(3, 4)
    // contributes 3 + 4 = 7. (`Circle` arm is also covered for exhaustiveness.)
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         ^ = () -> Num => <\n\
           s = Rect(3, 4)\n\
           s ?\n\
             | Circle(r) => r\n\
             | Rect(w, h) => w + h\n\
         >",
        7,
    );
}

#[test]
fn payload_sum_single_field_extracted() {
    // Single-payload variant: Circle(9) -> 9.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         ^ = () -> Num => <\n\
           s = Circle(9)\n\
           s ?\n\
             | Circle(r) => r\n\
             | Rect(w, h) => w + h\n\
         >",
        9,
    );
}

#[test]
fn function_over_sum_type_param_and_match() {
    // A function takes a sum-type parameter (`s :: Shape`) and dispatches on it —
    // exercises lowering a sum-type annotation to its tagged-union struct and passing
    // a constructed value as an argument. area(Rect(6, 7)) = 42.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         area = (s :: Shape) -> Num => s ?\n\
           | Circle(r)  => 3 * r * r\n\
           | Rect(w, h) => w * h\n\
         ^ = () -> Num => area(Rect(6, 7))",
        42,
    );
}

#[test]
fn result_still_works_as_a_normal_sum_type() {
    // The predefined Result behaves exactly as before, now via the general
    // sum-type mechanism: Ok(42) matched, payload doubled -> 84.
    assert_exit(
        "^ = () -> Num => <\n\
           outcome = Ok(42)\n\
           outcome ?\n\
             | Ok(x) => x * 2\n\
             | NotOk(e) => 0\n\
         >",
        84,
    );
}

#[test]
fn result_with_unit_payload_ok_dollar() {
    // `Ok($)` is the canonical "succeeded, no meaningful value" Result (like
    // `Result<(), E>`). A function returns `Ok($)` on success / `NotOk(code)` on
    // failure; matching `Ok(_)` (ignoring the unit payload) yields 0, `NotOk(c)`
    // yields the code. Here the success branch is taken -> exit 0.
    assert_exit(
        "validate = (n :: Num) -> Result => n <= 10 ? Ok($) : NotOk(n)\n\
         ^ = () -> Num => validate(5) ?\n\
           | Ok(_)     => 0\n\
           | NotOk(c)  => c",
        0,
    );
}

#[test]
fn result_with_unit_payload_notok_path() {
    // Same shape, failure branch: validate(20) -> NotOk(20) -> exit 20.
    assert_exit(
        "validate = (n :: Num) -> Result => n <= 10 ? Ok($) : NotOk(n)\n\
         ^ = () -> Num => validate(20) ?\n\
           | Ok(_)     => 0\n\
           | NotOk(c)  => c",
        20,
    );
}

#[test]
fn user_sum_with_unit_payload() {
    // `$` is a valid payload for a user sum type too, and may coexist with a
    // concrete-typed field at the same position (`Done($)` vs `Pending(Num)`).
    assert_exit(
        "Job = Done($) / Pending(Num)\n\
         ^ = () -> Num => <\n\
           j = Pending(7)\n\
           j ?\n\
             | Done(_)    => 0\n\
             | Pending(n) => n\n\
         >",
        7,
    );
}

#[test]
fn non_exhaustive_match_is_rejected() {
    // Missing the `Blue` arm (and no wildcard) over a user sum type must not compile.
    assert_type_error(
        "Color = Red / Green / Blue\n\
         classify = (c :: Color) -> Num => c ?\n\
           | Red => 0\n\
           | Green => 1",
    );
}

#[test]
fn non_builtin_payload_type_is_rejected() {
    // Payloads are built-in types only (Num / Text / Bool). A user type as a
    // payload (here the sum type referencing itself) is rejected.
    assert_type_error("Tree = Leaf / Node(Tree)");
}

#[test]
fn heterogeneous_payload_position_is_rejected() {
    // A sum type's payload slot has one shared representation per position, so two
    // variants disagreeing on a concrete type at the same position (Num vs Text)
    // would miscompile — the checker rejects it instead. (`$` may still coexist with
    // a concrete type; that's covered by `user_sum_with_unit_payload`.)
    assert_type_error("Mixed = A(Num) / B(Text)");
}

#[test]
fn duplicate_variant_names_are_rejected() {
    // Variant (constructor) names must be unique per scope — `Red` twice fails.
    assert_type_error("A = Red / Green\nB = Red / Blue");
}
