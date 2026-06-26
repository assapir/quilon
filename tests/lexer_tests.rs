// Integration tests for Quilon lexer

use quilon::lexer::{Lexer, TokenKind};

#[test]
fn test_hello_world() {
    let source = r#"
main = => <
  print "Hello, World!"
>
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    // Should have: main, =, =>, <, print, string, >, EOF
    assert!(tokens.len() >= 7);
    assert_eq!(tokens[0].text, "main");
    assert_eq!(tokens[1].kind, TokenKind::Assign);
    assert_eq!(tokens[2].kind, TokenKind::Arrow);
}

#[test]
fn test_factorial() {
    let source = r#"
factorial = n :: Num => n ?
  | 0 => 1
  | n => n * factorial (n - 1)
"#;

    let tokens = Lexer::tokenize(source).unwrap();
    let result = Lexer::tokenize(source);
    assert!(result.is_ok());

    // Check for key tokens
    assert!(tokens.iter().any(|t| t.text == "factorial"));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::TypeAnnotation));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Question));
    assert!(tokens.iter().filter(|t| t.kind == TokenKind::Pipe).count() >= 2);
}

#[test]
fn test_pipeline_expression() {
    let source = r#"
result = data
  |> filter .active
  |> map transform
  |> collect
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    // Should have 3 pipelines
    let pipeline_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Pipeline)
        .count();
    assert_eq!(pipeline_count, 3);
}

#[test]
fn test_mutable_variable() {
    // `:=` is the mutable bind/reassign operator (replaces the old `mut` keyword).
    let source = "counter := 0";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].text, "counter");
    assert_eq!(tokens[1].kind, TokenKind::MutAssign);
    assert!(matches!(tokens[2].kind, TokenKind::Number(_)));
}

#[test]
fn test_function_with_params() {
    let source = "add = (a :: Num, b :: Num) -> Num => a + b";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::ReturnArrow));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Arrow));
    // Two type annotations: a :: Num and b :: Num
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::TypeAnnotation)
            .count(),
        2
    );
}

#[test]
fn test_string_escapes() {
    let source = r#""hello\nworld\t\"\\""#;
    let tokens = Lexer::tokenize(source).unwrap();

    match &tokens[0].kind {
        TokenKind::String(s) => {
            assert!(s.contains('\n'));
            assert!(s.contains('\t'));
            assert!(s.contains('"'));
            assert!(s.contains('\\'));
        }
        _ => panic!("Expected string token"),
    }
}

#[test]
fn test_all_comparison_operators() {
    let source = "a == b && c != d && e <= f && g >= h";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::Eq));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Ne));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Le));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Ge));
    assert_eq!(
        tokens.iter().filter(|t| t.kind == TokenKind::And).count(),
        3
    );
}

#[test]
fn test_nested_blocks() {
    let source = r#"
outer = => <
  inner = => <
    print "nested"
  >
>
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    let open_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BlockOpen)
        .count();
    let close_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BlockClose)
        .count();

    assert_eq!(open_count, 2);
    assert_eq!(close_count, 2);
}

#[test]
fn test_array_syntax() {
    let source = "[1, 2, 3, 4, 5]";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].kind, TokenKind::BracketOpen);
    assert_eq!(
        tokens.iter().filter(|t| t.kind == TokenKind::Comma).count(),
        4
    );
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BracketClose));
}

#[test]
fn test_record_syntax() {
    let source = "{ name :: Text, age :: Num }";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].kind, TokenKind::BraceOpen);
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BraceClose));
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::TypeAnnotation)
            .count(),
        2
    );
}

#[test]
fn test_generic_type() {
    let source = "Result{T, E}";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].text, "Result");
    assert_eq!(tokens[1].kind, TokenKind::BraceOpen);
    // Result, {, T, ,, E, }, EOF -> index 5
    assert_eq!(tokens[5].kind, TokenKind::BraceClose);
}

#[test]
fn test_multiline_comment() {
    let source = r#"
x = 1
~ This is a comment
~ Another comment
y = 2
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    // Should have: x, =, 1, y, =, 2, EOF (comments skipped)
    assert!(tokens.iter().any(|t| t.text == "x"));
    assert!(tokens.iter().any(|t| t.text == "y"));
}

#[test]
fn test_ternary_operator() {
    let source = "result = x > 0 ? x : -x";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::Question));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Colon));
}
