//! Guaranteed self-tail-call optimization (M3): a function that returns a call to
//! ITSELF in tail position is lowered to a loop, so deep self-recursion runs in
//! constant stack instead of overflowing it.
//!
//! These tests drive the full pipeline (lex -> parse -> typecheck -> codegen -> JIT)
//! and assert the program's real exit code. The depth-1_000_000 cases are the
//! load-bearing ones: WITHOUT the optimization they recurse a million frames deep and
//! crash with a stack overflow; the fact that they return a deterministic value at all
//! is the guarantee. The remaining cases verify the transform preserves semantics for
//! tail calls reached through `?`/`|` match arms and nested `< >` blocks (the subtle
//! part), and that non-tail self-calls are left as ordinary recursion.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM's JIT / native-target init isn't thread-safe; cargo runs tests in parallel.
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

/// Assert the generated IR for `src` contains no recursive `call` to `callee` — i.e.
/// the self-recursion was lowered to a loop (a back-edge branch), not a stack call.
/// (The IR still names the function in its `define` line and at the initial call from
/// `^`; we check there is no `call <ret> @callee(` inside `callee` itself by counting:
/// the only legitimate `call ... @callee(` is the one in `^`.)
fn assert_no_self_call(src: &str, callee: &str) {
    let context = inkwell::context::Context::create();
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut codegen = quilon::codegen::CodeGenerator::with_oracle(&context, "test", &program)
        .expect("oracle setup failed");
    let ir = codegen.generate(&program).expect("codegen failed");
    let self_calls = ir.matches(&format!("@{}(", callee)).count();
    // Exactly one mention with `(` — the initial call from `^`; the `define` line uses
    // `@callee(` too, so allow up to 2 (define + initial call), but NONE may be a
    // back-edge recursive call. We assert the loop header is present as the positive
    // signal that the transform fired.
    assert!(
        ir.contains("tco_loop"),
        "expected a TCO loop header for {callee}; IR:\n{ir}"
    );
    // The recursive body must contain a back-edge branch to the loop header rather than
    // a self `call`. The `define` + single `^` call are the only `@callee(` occurrences.
    assert!(
        self_calls <= 2,
        "expected no recursive self-call to {callee} (found {self_calls} occurrences); IR:\n{ir}"
    );
}

/// Assert `src` compiles AND the generated module passes LLVM verification (codegen runs
/// `verify` internally and surfaces a failure as an `Err`). Used for programs that would
/// loop forever if run, but whose IR shape we still want to guard.
fn assert_compiles_clean(src: &str) {
    let context = inkwell::context::Context::create();
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut codegen = quilon::codegen::CodeGenerator::with_oracle(&context, "test", &program)
        .expect("oracle setup failed");
    codegen
        .generate(&program)
        .expect("codegen / module verification failed");
}

/// The headline guarantee: a ternary tail self-call recursing 1_000_000 deep returns
/// (does not overflow the stack) and computes the right value. `acc` cycles 0..250 so
/// the result is (steps) mod 251 = 1_000_000 mod 251 = 16.
#[test]
fn deep_ternary_tail_recursion_does_not_overflow() {
    assert_exit(
        "count = (n :: Num, acc :: Num) -> Num => \
           n == 0 ? acc : count(n - 1, acc == 250 ? 0 : acc + 1)\n\
         ^ = () -> Num => count(1000000, 0)",
        16,
    );
}

/// The same depth, but the tail self-call is reached through a `?`/`|` match arm — the
/// subtle path (the recursion lives in an arm body, not a ternary).
#[test]
fn deep_match_arm_tail_recursion_does_not_overflow() {
    assert_exit(
        "count = (n :: Num, acc :: Num) -> Num => n ? \
           | 0 => acc \
           | _ => count(n - 1, acc == 250 ? 0 : acc + 1)\n\
         ^ = () -> Num => count(1000000, 0)",
        16,
    );
}

/// The tail self-call is the tail expression of the function body `< >` block (so tail
/// position flows through the block to the ternary and into the call). Still must run in
/// constant stack. (A `< >` block can't start mid-expression, so the block is the whole
/// body and its tail is the recursing ternary — exercising the block-tail path.)
#[test]
fn deep_block_tail_recursion_does_not_overflow() {
    assert_exit(
        "count = (n :: Num, acc :: Num) -> Num => <\n\
           next = acc == 250 ? 0 : acc + 1\n\
           n == 0 ? acc : count(n - 1, next)\n\
         >\n\
         ^ = () -> Num => count(1000000, 0)",
        16,
    );
}

/// A simple tail countdown returning the accumulator, checked for a small deterministic
/// value (10 steps, +3 each) AND that codegen actually emitted the loop (no self-call).
#[test]
fn tail_recursion_computes_correct_value_and_loops() {
    let src = "sum = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : sum(n - 1, acc + 3)\n\
               ^ = () -> Num => sum(10, 0)";
    assert_exit(src, 30);
    assert_no_self_call(src, "sum");
}

/// A NON-tail self-call (the result is multiplied before returning) must NOT be loop-
/// lowered — it stays ordinary recursion and still computes correctly. factorial(5)=120.
#[test]
fn non_tail_recursion_still_works() {
    assert_exit(
        "fact = (n :: Num) -> Num => n <= 1 ? 1 : n * fact(n - 1)\n\
         ^ = () -> Num => fact(5)",
        120,
    );
}

/// Mutual / cross-function tail calls are explicitly OUT of scope for this milestone
/// (only SELF-tail-calls are optimized). A call to ANOTHER function in tail position
/// must remain a normal call and still compute the right value.
#[test]
fn tail_call_to_other_function_is_normal_call() {
    assert_exit(
        "twice = (n :: Num) -> Num => n + n\n\
         go = (n :: Num) -> Num => twice(n)\n\
         ^ = () -> Num => go(21)",
        42,
    );
}

/// Codegen-only: a body that is an UNCONDITIONAL tail self-call (`f(...)` with no base
/// case) must still produce a verifiable module — the back-edge `br` terminates the loop
/// block and no spurious `ret` is appended after it. (Running it would loop forever, so
/// we only check it compiles + verifies.) Guards a real double-terminator bug.
#[test]
fn unconditional_tail_self_call_verifies() {
    assert_compiles_clean("loop = (n :: Num) -> Num => loop(n)\n^ = () -> Num => loop(5)");
}

/// Codegen-only: a match all of whose arms tail-recurse leaves no value-producing path,
/// so the continuation block must be terminated as `unreachable` (not left dangling).
#[test]
fn all_match_arms_recurse_verifies() {
    assert_compiles_clean(
        "spin = (n :: Num) -> Num => n ? | 0 => spin(0) | _ => spin(n - 1)\n\
         ^ = () -> Num => spin(3)",
    );
}

/// Codegen-only: an `if`/ternary both of whose arms tail-recurse — the merge block has no
/// value-producing predecessor and must be terminated as `unreachable`.
#[test]
fn all_if_arms_recurse_verifies() {
    assert_compiles_clean(
        "spin = (n :: Num) -> Num => n == 0 ? spin(1) : spin(n - 1)\n\
         ^ = () -> Num => spin(3)",
    );
}

/// A tail self-call evaluates ALL its arguments against the CURRENT iteration's params
/// before overwriting any slot. Here the second arg reads `n`, which the first arg also
/// rebinds — a naive in-order overwrite would corrupt it. Sum 1..5 with a swap-style
/// dependency: count(5, 0) -> ... -> 15.
#[test]
fn tail_call_args_use_pre_update_param_values() {
    assert_exit(
        "count = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : count(n - 1, acc + n)\n\
         ^ = () -> Num => count(5, 0)",
        15,
    );
}
