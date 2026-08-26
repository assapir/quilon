// Higher-order functions across the pipeline: parsing a function type, type-checking a
// function-typed parameter (and rejecting an unannotated one), and running a user
// function that takes a closure and calls it.

use quilon::ast::{Item, Type};
use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{assert_exit, assert_parse_error, assert_type_error};

/// The `type_annotation` of the first parameter of the first top-level function.
fn first_parameter_type(src: &str) -> Type {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let function = program
        .items
        .iter()
        .find_map(|item| match item {
            Item::FunctionDeclaration(declaration) => Some(declaration),
            _ => None,
        })
        .expect("expected a function declaration");
    function.parameters[0]
        .type_annotation
        .clone()
        .expect("expected a parameter annotation")
}

#[test]
fn parses_function_type_annotation() {
    let ty = first_parameter_type("apply = (f :: (Num, Text) -> Bool, x :: Num) => x");
    assert_eq!(
        ty,
        Type::Function {
            parameters: vec![Type::Num, Type::Text],
            return_type: Box::new(Type::Bool),
        }
    );
}

#[test]
fn parses_no_argument_unit_function_type() {
    let ty = first_parameter_type("run = (action :: () -> $) => action()");
    assert_eq!(
        ty,
        Type::Function {
            parameters: vec![],
            return_type: Box::new(Type::Unit),
        }
    );
}

#[test]
fn parses_nested_function_type_parameter() {
    let ty = first_parameter_type("higher = (f :: ((Num) -> Bool, Num) -> Bool) => f");
    assert_eq!(
        ty,
        Type::Function {
            parameters: vec![
                Type::Function {
                    parameters: vec![Type::Num],
                    return_type: Box::new(Type::Bool),
                },
                Type::Num,
            ],
            return_type: Box::new(Type::Bool),
        }
    );
}

#[test]
fn rejects_curried_return_type() {
    // A function-typed RETURN position (currying) is deferred and rejected for now.
    assert_parse_error("apply = (f :: (Num) -> (Num) -> Bool) => f");
}

#[test]
fn unannotated_parameter_is_rejected() {
    // No `Num` default any more: an unannotated parameter that context cannot fill is an error.
    assert_type_error("add = (a, b) => a + b\n^ = () -> Num => add(1, 2)");
}

#[test]
fn function_typed_parameter_typechecks_and_calls() {
    // apply's `f` is a function value; calling it in the body is well-typed.
    assert_exit(
        "apply = (f :: (Num) -> Num, x :: Num) -> Num => f(x)\n\
         ^ = () -> Num => apply((n :: Num) => n + 1, 41)",
        42,
    );
}

#[test]
fn closure_passed_to_higher_order_function() {
    // A closure applied twice: twice(f, x) = f(f(x)); (x * 2) applied twice to 3 = 12.
    assert_exit(
        "twice = (f :: (Num) -> Num, x :: Num) -> Num => f(f(x))\n\
         ^ = () -> Num => twice((n :: Num) => n * 2, 3)",
        12,
    );
}

#[test]
fn predicate_typed_parameter() {
    // A `(Num) -> Bool` parameter used in a conditional.
    assert_exit(
        "keepIf = (p :: (Num) -> Bool, x :: Num) -> Num => p(x) ? x : 0\n\
         ^ = () -> Num => keepIf((n :: Num) => n > 5, 9)",
        9,
    );
}

#[test]
fn unit_returning_closure_called_for_effect() {
    // A `() -> $` closure captured by reference; the higher-order function calls it twice,
    // and the mutation escapes so the counter ends at 2.
    assert_exit(
        "run = (action :: () -> $) -> $ => <\n  action()\n  action()\n>\n\
         ^ = () -> Num => <\n  count := 0\n  tick = () -> $ => <\n    count := count + 1\n    $\n  >\n  run(tick)\n  count\n>",
        2,
    );
}
