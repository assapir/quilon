// String interpolation / format strings — full-pipeline coverage: lex -> parse ->
// typecheck -> codegen -> JIT execution. Interpolation renders each backtick hole via
// the value's `` ` `` operator (built-in default or user override) and concatenates.

use inkwell::context::Context;
use quilon::ast::{Expression, InterpolationPart};
use quilon::codegen::CodeGenerator;
use quilon::lexer::{Lexer, StrChunk, TokenKind};
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

mod common;
use common::{assert_exit, assert_exit_linked};

// ---- lexer -------------------------------------------------------------------------

#[test]
fn lexes_holes_and_doubled_backtick() {
    let tokens = Lexer::tokenize("\"a `x + 1` b `` c\"").unwrap();
    let TokenKind::String(chunks) = &tokens[0].kind else {
        panic!("expected a string token");
    };
    // Lit("a "), Hole("x + 1"), Lit(" b ` c") — the `` collapses to one literal backtick.
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0], StrChunk::Lit("a ".into()));
    match &chunks[1] {
        StrChunk::Hole { src, .. } => assert_eq!(src, "x + 1"),
        other => panic!("expected a hole, got {other:?}"),
    }
    assert_eq!(chunks[2], StrChunk::Lit(" b ` c".into()));
}

#[test]
fn hole_may_contain_a_nested_string() {
    // The `"` inside the hole must NOT end the outer string.
    let tokens = Lexer::tokenize("\"v `f(\"a\")`\"").unwrap();
    let TokenKind::String(chunks) = &tokens[0].kind else {
        panic!("expected a string token");
    };
    match &chunks[1] {
        StrChunk::Hole { src, .. } => assert_eq!(src, "f(\"a\")"),
        other => panic!("expected a hole, got {other:?}"),
    }
}

#[test]
fn bare_backtick_is_the_render_operator_token() {
    let tokens = Lexer::tokenize("` = () -> Text => < \"x\" >").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Backtick);
}

// ---- parser ------------------------------------------------------------------------

/// The entry point's body expression: `^`'s body is a block, and these tests are about
/// the one expression inside it.
fn entry_tail(src: &str) -> Expression {
    let program = parse(&Lexer::tokenize(src).unwrap()).unwrap();
    let body = program
        .items
        .into_iter()
        .find_map(|item| match item {
            quilon::ast::Item::FunctionDeclaration(f) if f.name == "^" => Some(f.body),
            _ => None,
        })
        .expect("entry point");
    let Expression::Block { mut statements, .. } = body else {
        panic!("expected a block body, got {body:?}");
    };
    match statements.pop().expect("a block with a tail expression") {
        quilon::ast::Statement::Expression(e) => e,
        other => panic!("expected a tail expression, got {other:?}"),
    }
}

#[test]
fn parses_interpolation_node_with_hole_expression() {
    let body = entry_tail("^ = () -> Text => < \"n `1 + 2`\" >");
    let Expression::Interpolation { parts, .. } = body else {
        panic!("expected an interpolation node, got {body:?}");
    };
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[0], InterpolationPart::Literal(s) if s == "n "));
    assert!(matches!(
        &parts[1],
        InterpolationPart::Hole(Expression::BinaryOperator { .. })
    ));
}

#[test]
fn plain_string_stays_a_string_node() {
    let body = entry_tail("^ = () -> Text => < \"plain\" >");
    assert!(matches!(body, Expression::String { .. }));
}

// ---- typechecker -------------------------------------------------------------------

fn type_checks(src: &str) -> Result<(), String> {
    let tokens = Lexer::tokenize(src).map_err(|e| e.to_string())?;
    let program = parse(&tokens).map_err(|e| e.to_string())?;
    TypeChecker::new()
        .check_program(&program)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn interpolation_type_checks_with_any_hole_type() {
    // Num, Bool, and Text holes are all renderable.
    type_checks("^ = () -> Text => < \"n `1` b `true` t `\"x\"`\" >").expect("should type check");
}

#[test]
fn backtick_override_must_return_text() {
    // A `` ` `` override returning Num is rejected.
    let err =
        type_checks("T = {\n  v :: Num,\n  ` = () -> Num => < it.v >\n}\n^ = () -> Num => < 0 >")
            .expect_err("a non-Text render override must be rejected");
    assert!(err.contains("render operator"), "unexpected error: {err}");
}

// ---- codegen -----------------------------------------------------------------------

#[test]
fn codegen_emits_render_intrinsics_for_holes() {
    let tokens = Lexer::tokenize("^ = () -> Text => < \"n `1` b `true`\" >").unwrap();
    let program = parse(&tokens).unwrap();
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type check failed");
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator.set_type_table(types);
    let ir = generator.generate(&program).expect("codegen");
    assert!(
        ir.contains("@__num_to_text"),
        "expected __num_to_text:\n{ir}"
    );
    assert!(
        ir.contains("@__bool_to_text"),
        "expected __bool_to_text:\n{ir}"
    );
}

// ---- run (JIT) ---------------------------------------------------------------------

/// A program that exits 1 iff the interpolation `interpolation` renders exactly to `expected`.
fn assert_renders(interpolation: &str, expected: &str) {
    assert_exit(
        &format!("^ = () -> Num => < ({interpolation} == \"{expected}\" ? 1 : 0) >"),
        1,
    );
}

#[test]
fn renders_num_expression_and_bool() {
    assert_renders("\"n `1 + 2`\"", "n 3");
    assert_renders("\"half `1 / 2`\"", "half 0.5");
    assert_renders("\"`true` `false`\"", "True False");
}

#[test]
fn doubled_backtick_is_one_literal_backtick() {
    // "a`b" is 3 bytes.
    assert_exit("^ = () -> Num => < \"a``b\".size >", 3);
}

#[test]
fn renders_record_default_and_override() {
    // Default: type name. Override: the user's `` ` `` body.
    assert_exit(
        "P = { x :: Num }\n^ = () -> Num => <\n  p :: P = P { x = 1 }\n  \"`p`\" == \"P\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "U = {\n  name :: Text,\n  ` = () -> Text => < \"U:`it.name`\" >\n}\n^ = () -> Num => <\n  u :: U = U { name = \"Ada\" }\n  \"`u`\" == \"U:Ada\" ? 1 : 0\n>",
        1,
    );
}

#[test]
fn renders_sum_variant_name() {
    assert_exit(
        "C = Red / Green / Blue\n^ = () -> Num => <\n  c :: C = Green\n  \"`c`\" == \"Green\" ? 1 : 0\n>",
        1,
    );
}

#[test]
fn renders_result_variant_name() {
    // The built-in `Result` now has one canonical `{ i8 tag, {ptr,i64} }` layout; rendering
    // reads only the tag, so a composite-payload Result still renders as its variant name.
    assert_exit(
        "^ = () -> Num => <\n  r :: Result = Ok(\"hi\")\n  \"`r`\" == \"Ok\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  r :: Result = NotOk([\"a\", \"b\"])\n  \"`r`\" == \"NotOk\" ? 1 : 0\n>",
        1,
    );
}

#[test]
fn renders_array_full_and_truncated() {
    // <= 10 elements: full. > 10: `[first <- last]`.
    assert_exit(
        "^ = () -> Num => <\n  a :: []Num = [1, 2, 3]\n  \"`a`\" == \"[1, 2, 3]\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  a :: []Num = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]\n  \"`a`\" == \"[1 <- 11]\" ? 1 : 0\n>",
        1,
    );
}

#[test]
fn renders_map_and_set_contents() {
    // A single-entry Map/Set renders deterministically (`[|k => v|]`/`[|e|]`); iteration
    // order over more than one entry is unspecified, so those cases only check membership
    // and separators, not an exact order.
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"ada\" => 36|]\n  \"`m`\" == \"[|ada => 36|]\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|=>|]\n  \"`m`\" == \"[|=>|]\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|1|]\n  \"`s`\" == \"[|1|]\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [||]\n  \"`s`\" == \"[||]\" ? 1 : 0\n>",
        1,
    );
    // Two entries: order-independent — both keys/elements present, comma-separated.
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Num => Num|] = [|1 => 10, 2 => 20|]\n  t :: Text = \"`m`\"\n  (t.contains(\"1 => 10\") && t.contains(\"2 => 20\") && t.contains(\", \") ? 1 : 0)\n>",
        1,
    );
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|1, 2|]\n  t :: Text = \"`s`\"\n  (t.contains(\"1\") && t.contains(\"2\") && t.contains(\", \") ? 1 : 0)\n>",
        1,
    );
}

#[test]
fn nested_interpolation_in_a_hole() {
    // A hole may contain a string literal with its OWN interpolation; offsets/bounds must
    // handle the nesting. `"inner `1`"` renders to "inner 1", so the outer is "outer inner 1".
    assert_renders("\"outer `\"inner `1`\"`\"", "outer inner 1");
}

#[test]
fn override_rendering_it_wholesale_terminates() {
    // A `` ` `` override that renders `it` wholesale must NOT recurse forever: the wholesale
    // `it` falls back to the built-in default (the type name). So U's `"U=`it`"` yields "U=U".
    assert_exit(
        "U = {\n  v :: Num,\n  ` = () -> Text => < \"U=`it`\" >\n}\n^ = () -> Num => <\n  u :: U = U { v = 1 }\n  \"`u`\" == \"U=U\" ? 1 : 0\n>",
        1,
    );
}

#[test]
fn print_renders_any_type() {
    // `print` accepts any single value now (routed through `` ` ``); a record prints fine.
    assert_exit_linked(
        "<< core.io\nP = { x :: Num }\n^ = () -> Num => <\n  io.print(P { x = 1 })\n  0\n>",
        0,
    );
}
