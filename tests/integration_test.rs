use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

/// Integration test: All major features working together
#[test]
fn test_all_features_integration() {
    let source = r#"
        ^ = () -> Num => <
            ~ For loops with blocks
            arr = [1, 2, 3]
            arr :> for n => <
                doubled = n * 2
                doubled
            >
            
            ~ Sum type constructors
            result = Ok(42)
            value = result ?
                | Ok(v) => v * 2
                | NotOk(e) => 0
            
            ~ Pattern matching on numbers
            check = 5 ?
                | 0 => 0
                | 5 => 100
                | _ => -1
            
            value + check
        >
    "#;

    // Parse
    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();

    // Type check
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Generate code
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "integration_test");
    let result = generator.generate(&program);
    assert!(result.is_ok(), "Codegen failed: {:?}", result.err());
}

/// Test that pattern matching properly extracts Result values
#[test]
fn test_result_pattern_extraction() {
    let source = r#"
        ^ = () -> Num => <
            success = Ok(100)
            failure = NotOk(404)
            
            v1 = success ?
                | Ok(x) => x + 1
                | NotOk(e) => 0
            
            v2 = failure ?
                | Ok(x) => x
                | NotOk(e) => e - 4
            
            v1 + v2
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

/// Test nested pattern matching
#[test]
fn test_nested_pattern_matching() {
    let source = r#"
        ^ = () -> Num => <
            ~ Pattern match on number to get Result,
            ~ then pattern match on Result
            x = 5
            
            step1 = x ?
                | 0 => 999
                | 5 => 42
                | _ => 0
            
            ~ Create Result and match
            result = Ok(step1)
            
            final = result ?
                | Ok(v) => v * 2
                | NotOk(e) => 0
            
            final
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

/// Test inline Result creation and matching
#[test]
fn test_inline_result_matching() {
    let source = r#"
        ^ = () -> Num => <
            ~ Create Result inline and immediately match
            value = (Ok(123)) ?
                | Ok(x) => x + 7
                | NotOk(e) => 0
            
            value
        >
    "#;

    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}
