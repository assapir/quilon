// String interpolation / format strings — full-pipeline coverage: lex -> parse ->
// typecheck -> codegen -> JIT execution. Interpolation renders each backtick hole via
// the value's `` ` `` operator (built-in default or user override) and concatenates.

use inkwell::context::Context;
use quilon::ast::{Expr, InterpPart};
use quilon::codegen::CodeGenerator;
use quilon::lexer::{Lexer, StrChunk, TokenKind};
use quilon::parser::{self, parse};
use quilon::typechecker::TypeChecker;
use std::path::Path;
use std::sync::Mutex;

// LLVM JIT / target init is not thread-safe; cargo runs tests in parallel.
static JIT_LOCK: Mutex<()> = Mutex::new(());

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
    let tokens = Lexer::tokenize("` = () -> Text => \"x\"").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Backtick);
}

// ---- parser ------------------------------------------------------------------------

#[test]
fn parses_interpolation_node_with_hole_expr() {
    let program = parse(&Lexer::tokenize("^ = () -> Text => \"n `1 + 2`\"").unwrap()).unwrap();
    // Dig out the entry-point body expression.
    let body = program
        .items
        .iter()
        .find_map(|item| match item {
            quilon::ast::Item::FunctionDecl(f) if f.name == "^" => Some(&f.body),
            _ => None,
        })
        .expect("entry point");
    let Expr::Interpolation { parts, .. } = body else {
        panic!("expected an interpolation node, got {body:?}");
    };
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[0], InterpPart::Lit(s) if s == "n "));
    assert!(matches!(&parts[1], InterpPart::Hole(Expr::BinOp { .. })));
}

#[test]
fn plain_string_stays_a_string_node() {
    let program = parse(&Lexer::tokenize("^ = () -> Text => \"plain\"").unwrap()).unwrap();
    let body = program
        .items
        .iter()
        .find_map(|item| match item {
            quilon::ast::Item::FunctionDecl(f) if f.name == "^" => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(matches!(body, Expr::String { .. }));
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
    type_checks("^ = () -> Text => \"n `1` b `true` t `\"x\"`\"").expect("should type check");
}

#[test]
fn backtick_override_must_return_text() {
    // A `` ` `` override returning Num is rejected.
    let err = type_checks("T = {\n  v :: Num,\n  ` = () -> Num => it.v\n}\n^ = () -> Num => 0")
        .expect_err("a non-Text render override must be rejected");
    assert!(err.contains("render operator"), "unexpected error: {err}");
}

// ---- codegen -----------------------------------------------------------------------

#[test]
fn codegen_emits_render_intrinsics_for_holes() {
    let tokens = Lexer::tokenize("^ = () -> Text => \"n `1` b `true`\"").unwrap();
    let program = parse(&tokens).unwrap();
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
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

/// Compile and run `src`, asserting the entry point yields `expected`.
fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code =
        quilon::jit::run_program(&program, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for:\n{src}");
}

/// As `assert_exit`, but resolves `<<` imports first (for programs using `core.io`).
fn assert_exit_linked(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let program = quilon::modules::link(program, Path::new(".")).expect("import linking failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code =
        quilon::jit::run_program(&program, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for:\n{src}");
}

/// A program that exits 1 iff the interpolation `interp` renders exactly to `expected`.
fn assert_renders(interp: &str, expected: &str) {
    assert_exit(
        &format!("^ = () -> Num => ({interp} == \"{expected}\" ? 1 : 0)"),
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
    assert_exit("^ = () -> Num => \"a``b\".size", 3);
}

#[test]
fn renders_record_default_and_override() {
    // Default: type name. Override: the user's `` ` `` body.
    assert_exit(
        "P = { x :: Num }\n^ = () -> Num => <\n  p :: P = P { x = 1 }\n  \"`p`\" == \"P\" ? 1 : 0\n>",
        1,
    );
    assert_exit(
        "U = {\n  name :: Text,\n  ` = () -> Text => \"U:`it.name`\"\n}\n^ = () -> Num => <\n  u :: U = U { name = \"Ada\" }\n  \"`u`\" == \"U:Ada\" ? 1 : 0\n>",
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
fn print_renders_any_type() {
    // `print` accepts any single value now (routed through `` ` ``); a record prints fine.
    assert_exit_linked(
        "<< core.io\nP = { x :: Num }\n^ = () -> Num => <\n  print(P { x = 1 })\n  0\n>",
        0,
    );
}
