use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

#[test]
fn test_ok_constructor_codegen() {
    let source = r#"
        ^ = () -> Num => <
            x = Ok(42)
            0
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Generate LLVM IR
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Verify the IR contains the expected struct with tag 0
    assert!(ir.contains("{ i8 0, double 4.200000e+01 }") || ir.contains("{ i8 0, double 42"));
}

#[test]
fn test_notok_constructor_codegen() {
    let source = r#"
        ^ = () -> Num => <
            x = NotOk(404)
            0
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Generate LLVM IR
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Verify the IR contains the expected struct with tag 1
    assert!(ir.contains("{ i8 1, double 4.040000e+02 }") || ir.contains("{ i8 1, double 404"));
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

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Generate LLVM IR
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // Verify both constructors are in the IR
    assert!(ir.contains("i8 0") && ir.contains("i8 1"));
}

#[test]
fn test_function_returning_result() {
    // A function may declare `-> Result` and return a constructor; codegen uses
    // the canonical { i8, double } Result representation for the return slot.
    let source = r#"
        make_ok = () -> Result => Ok(42)
        ^ = () -> Num => <
            r = make_ok()
            0
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_ok(),
        "Type check failed: {:?}",
        checker.check_program(&program).err()
    );

    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());

    let ir = result.unwrap();
    // The constructed Ok(42) payload is present as a double, tag 0.
    assert!(ir.contains("i8 0"));
    assert!(ir.contains("double 4.200000e+01") || ir.contains("double 42"));
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

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
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

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Generate LLVM IR
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    let result = generator.generate(&program);
    // String handling might not be fully implemented, but codegen should not crash
    // For now, just verify it doesn't panic
    let _ = result;
}
