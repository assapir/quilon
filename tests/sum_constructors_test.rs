use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

#[test]
fn test_ok_constructor_with_number() {
    let tokens = Lexer::tokenize("test = Ok(42)").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_ok_constructor_with_string() {
    let tokens = Lexer::tokenize("test = Ok(\"hello\")").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_notok_constructor_with_message() {
    let tokens = Lexer::tokenize("test = NotOk(\"error message\")").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_notok_constructor_with_number() {
    let tokens = Lexer::tokenize("test = NotOk(404)").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_ok_wrong_argument_count_zero() {
    let tokens = Lexer::tokenize("test = Ok()").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);
    assert!(result.is_err());
    assert!(format!("{:?}", result.unwrap_err()).contains("WrongNumberOfArguments"));
}

#[test]
fn test_ok_wrong_argument_count_two() {
    let tokens = Lexer::tokenize("test = Ok(1, 2)").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);
    assert!(result.is_err());
    assert!(format!("{:?}", result.unwrap_err()).contains("WrongNumberOfArguments"));
}

#[test]
fn test_notok_wrong_argument_count_zero() {
    let tokens = Lexer::tokenize("test = NotOk()").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);
    assert!(result.is_err());
    assert!(format!("{:?}", result.unwrap_err()).contains("WrongNumberOfArguments"));
}

#[test]
fn test_notok_wrong_argument_count_two() {
    let tokens = Lexer::tokenize("test = NotOk(1, 2)").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);
    assert!(result.is_err());
    assert!(format!("{:?}", result.unwrap_err()).contains("WrongNumberOfArguments"));
}

#[test]
fn test_sum_constructor_in_match() {
    // Pattern match returns different Results
    let tokens = Lexer::tokenize(
        r#"
        check = (x) => x ?
            | 0 => NotOk("zero")
            | _ => Ok(x)
    "#,
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_sum_constructor_as_return_value() {
    // Function that returns Result
    let tokens = Lexer::tokenize(
        r#"
        make_ok = () => Ok(42)
        x = make_ok()
    "#,
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_custom_sum_type_constructor() {
    // Custom sum type syntax not yet supported in parser
    let tokens = Lexer::tokenize(
        r#"
        Color = Red | Green | Blue
        test = Red
    "#,
    );
    // Parser doesn't support sum type declarations yet (pipe syntax)
    assert!(tokens.is_ok());
    let result = parse(&tokens.unwrap());
    assert!(result.is_err()); // Parse error expected
}

#[test]
fn test_custom_sum_type_with_fields() {
    // Custom sum type syntax not yet supported in parser
    let tokens = Lexer::tokenize(
        r#"
        Point = Cartesian(Num, Num) | Polar(Num, Num)
        test = Cartesian(3, 4)
    "#,
    );
    // Parser doesn't support sum type declarations yet (pipe syntax)
    assert!(tokens.is_ok());
    let result = parse(&tokens.unwrap());
    assert!(result.is_err()); // Parse error expected
}
