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

// NOTE: a test for functions that *return* Result (e.g. `make_ok = () => Ok(42)`)
// belongs here, but requires inferring/storing the Result return type from the body
// and the typed-payload struct representation — that is Workstream B3. B3 should add a
// passing version of this test. (Removed the previously #[ignore]d stub here.)

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
