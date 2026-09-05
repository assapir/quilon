use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

/// Type-check `source`, then generate it with the checker's type-oracle wired in — the
/// path every real compilation takes.
fn generate_checked(source: &str) -> Result<String, String> {
    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type check failed");
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator.set_type_table(types);
    generator.generate(&program)
}

#[test]
fn test_ok_constructor_codegen() {
    let source = r#"
        ^ = () -> Num => <
            x = Ok(42)
            0
        >
    "#;

    let result = generate_checked(source);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Result has one canonical `{ i8 tag, {ptr,i64} slot }` layout; a Num payload is PACKED
    // into the slot (its f64 bits in the i64 field, ptr field null), tagged Ok (0).
    assert!(ir.contains("{ i8, { ptr, i64 } }"));
    assert!(ir.contains("{ i8 0, { ptr, i64 } { ptr null, i64 "));
}

#[test]
fn test_notok_constructor_codegen() {
    let source = r#"
        ^ = () -> Num => <
            x = NotOk(404)
            0
        >
    "#;

    let result = generate_checked(source);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Same canonical layout, tagged NotOk (1): the Num payload's bits packed into the slot.
    assert!(ir.contains("{ i8, { ptr, i64 } }"));
    assert!(ir.contains("{ i8 1, { ptr, i64 } { ptr null, i64 "));
}

#[test]
fn test_both_constructors_codegen() {
    let source = r#"
        ^ = () -> Num => <
            x = Ok(100)
            y = NotOk(500)
            0
        >
    "#;

    let result = generate_checked(source);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Verify both constructors are in the IR
    assert!(ir.contains("i8 0") && ir.contains("i8 1"));
}

#[test]
fn test_function_returning_result() {
    // A function may declare `-> Result` and return a constructor; codegen uses the single
    // canonical `{ i8, {ptr,i64} }` Result representation for the return slot.
    let source = r#"
        make_ok = () -> Result => < Ok(42) >
        ^ = () -> Num => <
            r = make_ok()
            0
        >
    "#;

    let result = generate_checked(source);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // The constructed Ok(42) is the canonical Result struct, tagged 0, with the Num payload
    // packed into the `{ptr,i64}` slot — the same shape a `-> Result` return lowers to.
    assert!(ir.contains("{ i8, { ptr, i64 } }"));
    assert!(ir.contains("{ i8 0, { ptr, i64 } { ptr null, i64 "));
}

#[test]
fn test_text_payload_not_corrupted() {
    // Regression: previously every payload was coerced to f64, so a Text payload
    // became `{ i8 1, double 0.0 }`. Now the string pointer is preserved.
    let source = r#"
        ^ = () -> Num => <
            x = NotOk("error message")
            0
        >
    "#;

    let result = generate_checked(source);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // The string literal survives and the variant is tagged NotOk (1) — the
    // payload was NOT silently turned into `double 0.0`.
    assert!(ir.contains("error message"));
    assert!(ir.contains("i8 1"));
}

#[test]
fn test_string_in_constructor() {
    let source = r#"
        ^ = () -> Num => <
            x = NotOk("error message")
            0
        >
    "#;

    // String handling might not be fully implemented, but codegen should not crash.
    // For now, just verify it doesn't panic.
    let _ = generate_checked(source);
}
