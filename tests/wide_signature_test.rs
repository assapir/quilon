//! Wide definitions: a parameter list is read to its end, so what a definition MEANS
//! never depends on how many tokens it happens to span. A function stays a function
//! declaration — callable by name, including from its own body — at any width, and a
//! parameter list that is never closed still ends in a positioned parse error.

use quilon::ast::Item;
use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{assert_exit, assert_parse_error};

/// `wide = (p1 :: Num, …, pN :: Num) -> Num => p1 == 0 ? 7 : wide(0, …, 0)`, a function
/// of `parameters` annotated parameters that calls itself.
fn recursive_declaration(parameters: usize) -> String {
    let typed: Vec<String> = (1..=parameters).map(|i| format!("p{i} :: Num")).collect();
    let zeros: Vec<&str> = vec!["0"; parameters];
    format!(
        "wide = ({}) -> Num => p1 == 0 ? 7 : wide({})",
        typed.join(", "),
        zeros.join(", ")
    )
}

/// The declaration above plus an entry point that calls it with every argument `1`, so
/// the program recurses exactly once and exits 7.
fn recursive_program(parameters: usize) -> String {
    let ones: Vec<&str> = vec!["1"; parameters];
    format!(
        "{}\n\n^ = () -> Num => wide({})\n",
        recursive_declaration(parameters),
        ones.join(", ")
    )
}

fn parse_items(source: &str) -> Vec<Item> {
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    parser::parse(&tokens)
        .unwrap_or_else(|e| panic!("parsing failed: {e}\n{source}"))
        .items
}

fn parse_error(source: &str) -> quilon::parser::ast_parser::ParseError {
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    parser::parse(&tokens).expect_err("expected a parse error")
}

#[test]
fn a_thirty_parameter_declaration_parses_as_a_function() {
    let items = parse_items(&recursive_declaration(30));
    match &items[0] {
        Item::FunctionDeclaration(function) => {
            assert_eq!(function.name, "wide");
            assert_eq!(function.parameters.len(), 30);
            assert!(
                function
                    .parameters
                    .iter()
                    .all(|p| p.type_annotation.is_some()),
                "every parameter keeps its annotation"
            );
        }
        other => panic!("expected a function declaration, got {other:?}"),
    }
}

/// The shape that used to become a variable holding a lambda: wide enough that the
/// declaration scan gave up, narrow enough that the lambda scan did not. The name then
/// did not exist inside its own body.
#[test]
fn a_seventeen_parameter_function_can_call_itself() {
    assert_exit(&recursive_program(17), 7);
}

#[test]
fn a_thirty_parameter_function_can_call_itself() {
    assert_exit(&recursive_program(30), 7);
}

/// The opposite reading has to survive too: a long parenthesized expression is a value,
/// not a parameter list, however many tokens it spans.
#[test]
fn a_long_parenthesized_expression_stays_a_value() {
    let terms: Vec<&str> = vec!["1"; 50];
    assert_exit(&format!("^ = () -> Num => ({})\n", terms.join(" + ")), 50);
}

/// A parameter list that is never closed must still end — at the end of the token stream
/// — and report where the trouble is, not run off into whatever follows.
#[test]
fn an_unclosed_parameter_list_is_a_positioned_parse_error() {
    let source = "wide = (a :: Num, b :: Num -> Num => a + b\n\n^ = () -> Num => 0\n";
    let error = parse_error(source);
    let first_line_end = source.find('\n').expect("source has a newline") as u32;
    assert!(
        error.span.start <= first_line_end,
        "error points past the offending line (offset {}, line ends at {first_line_end}): {}",
        error.span.start,
        error.message
    );
}

/// The same for an unbalanced parenthesis in expression position, which the lambda scan
/// also has to walk past.
#[test]
fn an_unbalanced_expression_parenthesis_is_a_parse_error() {
    let terms: Vec<&str> = vec!["1"; 50];
    assert_parse_error(&format!("^ = () -> Num => (1 + ({}\n", terms.join(" + ")));
}

/// A stray token inside a parameter list is reported AT that token. Annotated parameters
/// say what the parentheses are, so the list is still read as a list — a malformed one —
/// rather than falling back to the expression reading, which would blame a `::` several
/// parameters earlier.
#[test]
fn a_stray_token_in_a_parameter_list_is_reported_where_it_is() {
    let source = "add = (a :: Num, b :: 2) -> Num => a + b\n";
    let error = parse_error(source);
    assert!(
        error.message.contains("Expected type"),
        "expected the type position to be blamed, got: {}",
        error.message
    );
    assert_eq!(
        &source[error.span.start as usize..error.span.end as usize],
        "2"
    );

    let source = "add = (a :: Num, b = 2) -> Num => a + b\n";
    let error = parse_error(source);
    assert_eq!(
        &source[error.span.start as usize..error.span.end as usize],
        "="
    );
}

/// A lambda in parentheses is not a parameter list, however its body reads: the `=>`
/// inside the parentheses already opened one.
#[test]
fn a_parenthesized_lambda_is_still_an_expression() {
    assert_exit(
        "^ = () -> Num => <\n  f = (x :: Num => x + 1)\n  f(4)\n>",
        5,
    );
}
