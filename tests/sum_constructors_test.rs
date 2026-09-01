use quilon::lexer::Lexer;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;

#[test]
fn test_ok_constructor_with_number() {
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  test = Ok(42)\n  0\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_ok_constructor_with_string() {
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  test = Ok(\"hello\")\n  0\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_notok_constructor_with_message() {
    let tokens =
        Lexer::tokenize("^ = () -> Num => <\n  test = NotOk(\"error message\")\n  0\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_notok_constructor_with_number() {
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  test = NotOk(404)\n  0\n>").unwrap();
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
        check = (x :: Num) => <
          x ?
              | 0 => NotOk("zero")
              | _ => Ok(x)
        >
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
        make_ok = () => < Ok(42) >
        ^ = () -> Num => <
          x = make_ok()
          0
        >
    "#,
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_custom_sum_type_constructor() {
    // User-defined nullary sum type with the `/` separator (LOCKED design).
    let tokens = Lexer::tokenize(
        r#"
        Color = Red / Green / Blue
        ^ = () -> Num => <
          test = Red
          0
        >
    "#,
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_custom_sum_type_with_fields() {
    // User-defined sum type with payload variants, separated by `/`.
    let tokens = Lexer::tokenize(
        r#"
        Point = Cartesian(Num, Num) / Polar(Num, Num)
        ^ = () -> Num => <
          test = Cartesian(3, 4)
          0
        >
    "#,
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_slash_remains_division_for_values() {
    // Disambiguation: `/` between lowercase values is division, NOT a sum type.
    let tokens = Lexer::tokenize("half = a / b").unwrap();
    let program = parse(&tokens).unwrap();
    // Parses as a value binding (division), so it's a VariableDeclaration, not a TypeDeclaration.
    assert!(matches!(
        program.items.first(),
        Some(quilon::ast::Item::VariableDeclaration(_))
    ));
}

#[test]
fn test_slash_with_capitalized_left_but_nonconstructor_right_is_division() {
    // A sum type requires BOTH operands to be Capitalized constructor names. A
    // Capitalized left operand divided by a number/lowercase value is still division,
    // not a misparsed one-variant sum type.
    for src in ["Max = Min / 2", "Max = Min / count"] {
        let tokens = Lexer::tokenize(src).unwrap();
        let program = parse(&tokens).unwrap();
        assert!(
            matches!(
                program.items.first(),
                Some(quilon::ast::Item::VariableDeclaration(_))
            ),
            "`{src}` should parse as division (a VariableDeclaration), not a sum type"
        );
    }
}
