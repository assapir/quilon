//! Every `?`/`|` match is total, and every pattern in one names something real.
//!
//! The two holes this pins shut: a match on a non-sum scrutinee used to need no coverage at
//! all (`n ? | 0 => 1 | 1 => 2` type-checked, ran, and yielded whatever the result slot
//! happened to hold), and a constructor pattern used to be accepted whatever it named, only
//! to die in codegen with no location. Both are compile errors now, and the no-match edge
//! codegen still emits — the backstop for what the checker cannot prove — aborts rather than
//! loading a slot no arm ever wrote.

mod common;

use common::{assert_exit, build_and_run_native, tool_available, type_error_message};
use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

/// The LLVM IR for `source`, which must type-check first.
fn emit_ir(source: &str) -> String {
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let program = parse(&tokens).expect("parsing failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");

    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator.generate(&program).expect("codegen failed")
}

#[test]
fn a_non_exhaustive_match_on_a_non_sum_is_a_compile_error() {
    let message = type_error_message("^ = () -> Num => <\n  n = 5\n  n ? | 0 => 1 | 1 => 2\n>");
    assert!(
        message.contains("this match on Num is not exhaustive"),
        "the diagnostic must name the scrutinee's type, got: {message}"
    );
    assert!(
        message.contains("'_'"),
        "the diagnostic must point at the fix, got: {message}"
    );
}

#[test]
fn a_wildcard_arm_covers_the_values_no_arm_lists() {
    assert_exit(
        "^ = () -> Num => <\n  n = 5\n  n ? | 0 => 1 | 5 => 50 | _ => 99\n>",
        50,
    );
    assert_exit(
        "^ = () -> Num => <\n  n = 7\n  n ? | 0 => 1 | 5 => 50 | _ => 99\n>",
        99,
    );
}

#[test]
fn a_binding_arm_covers_them_too() {
    // An identifier pattern is irrefutable — it binds whatever reaches it — so it makes a
    // match total exactly as `_` does.
    assert_exit(
        "^ = () -> Num => <\n  n = 7\n  n ? | 0 => 1 | rest => rest\n>",
        7,
    );
}

#[test]
fn a_text_match_is_covered_by_its_binding_arm() {
    // No pattern names a string, so a catch-all is the only way to cover a `Text` — and it
    // does cover it: this compiles and runs.
    assert_exit(
        "^ = () -> Num => <\n  t = \"abc\"\n  t ? | s => s.size\n>",
        3,
    );
}

#[test]
fn a_sum_match_missing_a_variant_names_it() {
    let message = type_error_message(
        "Color = Red / Green / Blue\n^ = () -> Num => <\n  c :: Color = Green\n  c ? | Red => 0 | Green => 1\n>",
    );
    assert!(
        message.contains("this match on Color is not exhaustive"),
        "the diagnostic must name the sum, got: {message}"
    );
    assert!(
        message.contains("'Blue'"),
        "the diagnostic must name the uncovered variant, got: {message}"
    );
}

#[test]
fn an_unknown_constructor_is_a_compile_error() {
    let message = type_error_message(
        "Color = Red / Green / Blue\n^ = () -> Num => <\n  c :: Color = Green\n  c ? | Purple => 0 | _ => 1\n>",
    );
    assert!(
        message.contains("'Purple' is not a variant of 'Color'"),
        "the diagnostic must name the constructor and the sum, got: {message}"
    );
    assert!(
        message.contains("'Red', 'Green', 'Blue'"),
        "the diagnostic must list the variants there are, got: {message}"
    );
}

#[test]
fn a_constructor_pattern_on_a_non_sum_is_a_compile_error() {
    let message = type_error_message("^ = () -> Num => <\n  n = 5\n  n ? | Ok(x) => x | _ => 0\n>");
    assert!(
        message.contains("'Ok'") && message.contains("Num"),
        "the diagnostic must name the constructor and the scrutinee's type, got: {message}"
    );
}

/// The backstop, in the IR: past the last arm's check, control goes to an abort — never to
/// the continuation, which loads a result slot only a matching arm writes.
#[test]
fn the_no_match_edge_aborts_instead_of_loading_an_unwritten_slot() {
    let ir = emit_ir("^ = () -> Num => <\n  n = 5\n  n ? | 0 => 1 | _ => 2\n>");
    assert!(
        ir.contains("match_no_arm:"),
        "a match must emit its no-arm block, got:\n{ir}"
    );
    assert!(
        ir.contains("call void @__match_fail("),
        "the no-arm block must fail loudly, got:\n{ir}"
    );
    assert_no_check_falls_through_to_the_continuation(&ir);
}

/// The same edge on the tail-call path, which lowers matches through its own routine.
#[test]
fn a_tail_position_match_aborts_on_its_no_match_edge_too() {
    let ir = emit_ir(
        "countdown = (n :: Num) -> Num => n ? | 0 => 0 | _ => countdown(n - 1)\n^ = () -> Num => countdown(3)",
    );
    assert!(
        ir.contains("call void @__match_fail("),
        "a tail-position match must fail loudly on its no-arm edge, got:\n{ir}"
    );
    assert_no_check_falls_through_to_the_continuation(&ir);
}

/// No arm's pattern test may branch to `match_cont` when it fails: that block loads the
/// result slot, and reaching it without a store is the read of uninitialized stack this
/// whole change exists to remove.
fn assert_no_check_falls_through_to_the_continuation(ir: &str) {
    for line in ir.lines().filter(|line| line.contains("br i1 ")) {
        let branch = line.trim();
        assert!(
            !branch.ends_with("label %match_cont"),
            "a failed pattern test falls through to the result load: {branch}"
        );
    }
}

#[test]
fn a_covered_match_still_builds_and_runs_natively() {
    if !tool_available("clang") {
        eprintln!("skipping the native build: clang is not on PATH");
        return;
    }
    let (code, _) = build_and_run_native(
        "match_covered",
        "^ = () -> Num => <\n  n = 5\n  n ? | 0 => 1 | 5 => 42 | _ => 99\n>",
    );
    assert_eq!(code, 42, "the matching arm's value is the exit code");
}
