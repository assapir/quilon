//! The parameter limit: a function, method or lambda declares at most ten parameters, and
//! the eleventh is a compile error naming the limit and pointing at a record. A record type
//! is the escape hatch, so it carries no such limit.
//!
//! The limit is only useful if a too-wide definition actually REACHES it. These also pin
//! that down: a definition well past the limit reports the limit, never `Undefined variable`
//! from being re-read as a variable holding a lambda, and never a confused
//! `Expected ParenClose`.

use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::assert_exit;

/// The text every rejection must carry.
const LIMIT_MESSAGE: &str = "a function takes at most 10 parameters — group them into a record type and take that \
     record as one parameter instead";

/// `wide = (p1 :: Num, …, pN :: Num) -> Num => p1 == 0 ? 7 : wide(0, …, 0)`, a function of
/// `parameters` annotated parameters that calls itself, plus an entry point calling it with
/// every argument `1` — so a legal one recurses exactly once and exits 7.
fn recursive_program(parameters: usize) -> String {
    let typed: Vec<String> = (1..=parameters).map(|i| format!("p{i} :: Num")).collect();
    let zeros: Vec<&str> = vec!["0"; parameters];
    let ones: Vec<&str> = vec!["1"; parameters];
    format!(
        "wide = ({}) -> Num => < p1 == 0 ? 7 : wide({}) >\n\n^ = () -> Num => < wide({}) >\n",
        typed.join(", "),
        zeros.join(", "),
        ones.join(", ")
    )
}

/// Parse `source`, expect the limit error, and return the source text it points at.
fn limit_error_at(source: &str) -> String {
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let error = parser::parse(&tokens).expect_err("expected the parameter limit to be reported");
    assert_eq!(
        error.message, LIMIT_MESSAGE,
        "wrong diagnostic for:\n{source}"
    );
    source[error.span.start as usize..error.span.end as usize].to_string()
}

#[test]
fn ten_parameters_are_accepted_and_the_function_can_call_itself() {
    assert_exit(&recursive_program(10), 7);
}

#[test]
fn the_eleventh_parameter_is_reported_where_it_is_written() {
    assert_eq!(limit_error_at(&recursive_program(11)), "p11");
}

/// Well past the limit: the scan still recognizes the parameter list, so the limit is what
/// gets reported. Before, this width was re-read as a variable holding a lambda (making the
/// recursive call fail with `Undefined variable 'wide'`) or rejected with a parenthesis
/// complaint that said nothing about the real problem.
#[test]
fn a_thirty_parameter_function_reports_the_limit_not_a_misparse() {
    assert_eq!(limit_error_at(&recursive_program(30)), "p11");
}

#[test]
fn a_lambda_is_held_to_the_limit_too() {
    let parameters: Vec<String> = (1..=11).map(|i| format!("p{i} :: Num")).collect();
    let source = format!(
        "^ = () -> Num => <\n  f = ({}) -> Num => < p1 >\n  f(1)\n>\n",
        parameters.join(", ")
    );
    assert_eq!(limit_error_at(&source), "p11");
}

#[test]
fn a_method_is_held_to_the_limit_too() {
    let parameters: Vec<String> = (1..=11).map(|i| format!("p{i} :: Num")).collect();
    let source = format!(
        "Box = {{\n  size :: Num\n  fill = ({}) -> Num => < p1 >\n}}\n\n^ = () -> Num => < 0 >\n",
        parameters.join(", ")
    );
    assert_eq!(limit_error_at(&source), "p11");
}

/// The escape hatch the diagnostic points at has to be unlimited, or the advice is empty.
#[test]
fn a_record_type_carries_no_field_limit() {
    let names: Vec<String> = (1..=13).map(|i| format!("f{i}")).collect();
    let fields: Vec<String> = names.iter().map(|n| format!("  {n} :: Num")).collect();
    let values: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{n} = {}", i + 1))
        .collect();
    let source = format!(
        "Wide = {{\n{}\n}}\n\ntotalOf = (w :: Wide) -> Num => < w.f13 >\n\n\
         ^ = () -> Num => < totalOf(Wide {{ {} }}) >\n",
        fields.join("\n"),
        values.join(", ")
    );
    assert_exit(&source, 13);
}

/// The limit must not misfire on a parenthesized expression, however many terms it has —
/// that is a value, not a parameter list.
#[test]
fn a_long_parenthesized_expression_is_unaffected() {
    let terms: Vec<&str> = vec!["1"; 50];
    assert_exit(
        &format!("^ = () -> Num => < ({}) >\n", terms.join(" + ")),
        50,
    );
}

/// A parameter list that is never closed must still end — at the end of the token stream —
/// and report on the offending line rather than running off into whatever follows.
#[test]
fn an_unclosed_parameter_list_is_a_positioned_parse_error() {
    let source = "wide = (a :: Num, b :: Num -> Num => a + b\n\n^ = () -> Num => < 0 >\n";
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let error = parser::parse(&tokens).expect_err("an unclosed `(` must be rejected");
    let first_line_end = source.find('\n').expect("source has a newline") as u32;
    assert!(
        error.span.start <= first_line_end,
        "error points past the offending line (offset {}, line ends at {first_line_end}): {}",
        error.span.start,
        error.message
    );
}

/// A stray token inside a parameter list is still reported at that token.
#[test]
fn a_stray_token_in_a_parameter_list_is_reported_where_it_is() {
    let source = "add = (a :: Num, b :: 2) -> Num => < a + b >\n";
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let error = parser::parse(&tokens).expect_err("expected a parse error");
    assert!(
        error.message.contains("Expected type"),
        "expected the type position to be blamed, got: {}",
        error.message
    );
    assert_eq!(
        &source[error.span.start as usize..error.span.end as usize],
        "2"
    );
}
