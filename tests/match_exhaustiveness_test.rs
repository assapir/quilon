//! Every `?`/`|` match is total, and every pattern in one names something real.
//!
//! The two holes this pins shut: a match on a non-sum scrutinee used to need no coverage at
//! all (`n ? | 0 => 1 | 1 => 2` type-checked, ran, and yielded whatever the result slot
//! happened to hold), and a constructor pattern used to be accepted whatever it named, only
//! to die in codegen with no location. Both are compile errors now, and the no-match edge
//! codegen still emits — the backstop for what the checker cannot prove — aborts rather than
//! loading a slot no arm ever wrote.

mod common;

use common::{assert_exit, type_error_message};
use inkwell::context::Context;
use quilon::codegen::CodeGenerator;

/// The LLVM IR for `source`, generated the way the compiler generates it — through the
/// shared front end, with the type oracle and source map codegen reads.
fn emit_ir(source: &str) -> String {
    let (program, types, defer, sources) = common::front_end(source, None);
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator.set_type_table(types);
    generator.set_defer_info(defer);
    generator.set_source_map(sources);
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
        message.contains("help: add a `_` arm"),
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
/// the continuation, which loads a result slot only a matching arm writes. Both matches
/// here end in a REFUTABLE arm, so both carry the edge; a second match in the same function
/// also pins the check below against LLVM's renaming of a repeated block name.
#[test]
fn the_no_match_edge_aborts_instead_of_loading_an_unwritten_slot() {
    let ir = emit_ir(
        "Color = Red / Green\n^ = () -> Num => <\n  c :: Color = Green\n  first = c ? | Red => 0 | Green => 1\n  second = c ? | Red => 2 | Green => 3\n  first + second\n>",
    );
    assert_eq!(
        ir.matches("call void @__match_fail(").count(),
        2,
        "each match's no-arm block must fail loudly, got:\n{ir}"
    );
    assert_no_check_falls_through_to_the_continuation(&ir);
}

/// The same edge on the tail-call path, which lowers matches through its own routine.
#[test]
fn a_tail_position_match_aborts_on_its_no_match_edge_too() {
    let ir = emit_ir(
        "Color = Red / Green\ncount = (c :: Color, n :: Num) -> Num => < c ? | Red => n | Green => count(Red, n + 1) >\n^ = () -> Num => < count(Green, 0) >",
    );
    assert!(
        ir.contains("call void @__match_fail("),
        "a tail-position match must fail loudly on its no-arm edge, got:\n{ir}"
    );
    assert_no_check_falls_through_to_the_continuation(&ir);
}

/// A last arm that takes everything leaves nothing past it, and codegen emits no edge —
/// no dead block, and no `Site` interning the match's source line for a report that can
/// never print. The abort is the backstop for a real edge, not a tax on every match.
#[test]
fn a_match_ending_in_a_catch_all_emits_no_abort_at_all() {
    let ir = emit_ir("^ = () -> Num => <\n  n = 5\n  n ? | 0 => 1 | _ => 2\n>");
    assert!(
        !ir.contains("@__match_fail"),
        "an always-matching last arm needs no abort, got:\n{ir}"
    );
    assert_no_check_falls_through_to_the_continuation(&ir);
}

/// No arm's pattern test may branch to a continuation when it fails: that block loads the
/// result slot, and reaching it without a store is the read of uninitialized stack this
/// whole change exists to remove. LLVM uniquifies a repeated block name (`match_cont8`),
/// so the target is compared by prefix rather than by the bare name.
fn assert_no_check_falls_through_to_the_continuation(ir: &str) {
    for branch in ir
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("br i1 "))
    {
        let target = branch.rsplit("label %").next().unwrap_or_default();
        assert!(
            !target.starts_with("match_cont"),
            "a failed pattern test falls through to the result load: {branch}"
        );
    }
}
