// Higher-order functions across the pipeline: parsing a function type, type-checking a
// function-typed parameter (and rejecting an unannotated one), and running a user
// function that takes a closure and calls it.

use quilon::ast::{FunctionDeclaration, Item, Type};
use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{assert_exit, assert_parse_error, assert_type_error, type_error_message};

/// The first top-level function declaration of `src`.
fn first_function(src: &str) -> FunctionDeclaration {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    program
        .items
        .iter()
        .find_map(|item| match item {
            Item::FunctionDeclaration(declaration) => Some(declaration.clone()),
            _ => None,
        })
        .expect("expected a function declaration")
}

/// The `type_annotation` of the first parameter of the first top-level function.
fn first_parameter_type(src: &str) -> Type {
    first_function(src).parameters[0]
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
fn binding_annotation_that_is_a_function_type_is_the_whole_signature() {
    // `f :: (Num) -> Text = …` states what `f` IS, not what it returns: the parameter takes
    // its type from the slot, and the function's return is the annotation's.
    let function = first_function("f :: (Num) -> Text = (n) => \"x\"");
    assert_eq!(function.parameter_type(0), Some(&Type::Num));
    assert_eq!(function.declared_return_type(), Some(&Type::Text));
}

#[test]
fn binding_annotation_that_is_not_a_function_type_is_the_return_type() {
    let function = first_function("f :: Num = (n :: Num) => n * 2");
    assert_eq!(function.parameter_type(0), Some(&Type::Num));
    assert_eq!(function.declared_return_type(), Some(&Type::Num));
}

#[test]
fn written_annotations_win_over_the_binding_type() {
    // Read at the AST level, where the two may still disagree — the type checker is what
    // rejects that (see `binding_type_and_*_must_agree`).
    let function = first_function("f :: (Num) -> Num = (n :: Text) -> Bool => true");
    assert_eq!(function.parameter_type(0), Some(&Type::Text));
    assert_eq!(function.declared_return_type(), Some(&Type::Bool));
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
fn returning_a_function_is_rejected() {
    // Taking a function is supported; handing one back across the call boundary is deferred.
    assert_type_error("pick = (f :: (Num) -> Num) => f\n^ = () -> Num => 0");
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
fn capturing_nested_higher_order_function() {
    // A nested higher-order function that ALSO captures an outer binding is lifted through
    // the closure machinery; its function-typed parameter must still be callable there.
    assert_exit(
        "^ = () -> Num => <\n  base = 1\n  g = (h :: (Num) -> Num) -> Num => h(base)\n  g((n :: Num) => n + 41)\n>",
        42,
    );
}

#[test]
fn method_with_function_typed_parameter() {
    // A record method may take a function value and call it on a field of the receiver.
    assert_exit(
        "Calc = {\n  factor :: Num,\n  applyTo = (f :: (Num) -> Num) -> Num => f(it.factor)\n}\n\
         ^ = () -> Num => <\n  c = Calc { factor = 41 }\n  c.applyTo((n :: Num) => n + 1)\n>",
        42,
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
fn lambda_argument_takes_its_parameter_types_from_the_signature() {
    // `apply`'s `(Num) -> Num` parameter types `n`; the lambda writes no annotation.
    assert_exit(
        "apply = (x :: Num, f :: (Num) -> Num) -> Num => f(x)\n\
         ^ = () -> Num => apply(41, (n) => n + 1)",
        42,
    );
}

#[test]
fn inferred_lambda_parameter_is_not_num_by_default() {
    // The target says `Text`, so `s` is a `Text` — a `Num` default would fail to compile
    // (`.size` is not a Num field) instead of quietly picking the wrong type.
    assert_exit(
        "measure = (t :: Text, f :: (Text) -> Num) -> Num => f(t)\n\
         ^ = () -> Num => measure(\"hello\", (s) => s.size)",
        5,
    );
}

#[test]
fn inferred_lambda_parameter_through_a_pipe() {
    // `x |> f(a)` is `f(x, a)`, so the piped call states the same target type.
    assert_exit(
        "apply = (x :: Num, f :: (Num) -> Num) -> Num => f(x)\n\
         ^ = () -> Num => 20 |> apply((n) => n * 2)",
        40,
    );
}

#[test]
fn inferred_lambda_parameter_for_a_method() {
    // A method's function-typed parameter types the lambda its call is given.
    assert_exit(
        "Calc = {\n  factor :: Num,\n  applyTo = (f :: (Num) -> Num) -> Num => f(it.factor)\n}\n\
         ^ = () -> Num => <\n  c = Calc { factor = 41 }\n  c.applyTo((n) => n + 1)\n>",
        42,
    );
}

#[test]
fn binding_declares_the_function_type_it_holds() {
    // `bump :: (Num) -> Num = …` states the whole signature, so `n` needs no annotation
    // and the declared return still stands.
    assert_exit(
        "^ = () -> Num => <\n  bump :: (Num) -> Num = (n) => n + 2\n  bump(40)\n>",
        42,
    );
}

#[test]
fn declared_function_type_binding_at_the_top_level() {
    assert_exit(
        "shout :: (Text) -> Num = (t) => t.size\n^ = () -> Num => shout(\"abcd\")",
        4,
    );
}

#[test]
fn explicit_annotation_wins_over_the_binding_type() {
    // A written annotation is still legal in every one of these positions.
    assert_exit(
        "apply = (x :: Num, f :: (Num) -> Num) -> Num => f(x)\n\
         ^ = () -> Num => apply(41, (n :: Num) -> Num => n + 1)",
        42,
    );
}

#[test]
fn binding_type_and_parameter_annotation_must_agree() {
    assert_type_error("f :: (Num) -> Num = (n :: Text) => n.size\n^ = () -> Num => 0");
}

#[test]
fn binding_type_and_return_annotation_must_agree() {
    // Writing both must not let the `->` quietly override the type the binding declares.
    assert_type_error(
        "f :: (Num) -> Num = (n) -> Text => \"abc\"\n\
         ^ = () -> Num => <\n  v = f(1)\n  v.size\n>",
    );
}

#[test]
fn binding_type_and_parameter_count_must_agree() {
    // Reported even where a parameter is unannotated — the arity is what is wrong, not the
    // missing annotation.
    for source in [
        "f :: (Num, Num) -> Num = (a :: Num) => a\n^ = () -> Num => 0",
        "f :: (Num, Num) -> Num = (a) => a\n^ = () -> Num => 0",
    ] {
        let message = type_error_message(source);
        assert!(
            message.contains("'f' takes 1 parameter, but the function type it must match takes 2"),
            "unexpected message: {message}"
        );
    }
}

#[test]
fn lambda_of_another_arity_than_the_target_is_an_arity_error() {
    // The position DOES state a function type, so the diagnostic must say the arity is
    // wrong rather than claim nothing stated a type.
    let message = type_error_message(
        "apply = (x :: Num, f :: (Num) -> Num) -> Num => f(x)\n\
         ^ = () -> Num => apply(1, (a, b) => a)",
    );
    assert!(
        message.contains(
            "this lambda takes 2 parameters, but the function type it must match takes 1"
        ),
        "unexpected message: {message}"
    );
}

#[test]
fn contextually_typed_parameter_resolves_its_render_member() {
    // `p` is typed only by `describeIt`'s signature, and rendering it in a hole has to find
    // the `` ` `` member of THAT type — the inferred parameter type must reach the render
    // path as a written annotation would. "P7!" is 3 graphemes.
    assert_exit(
        "Point = {\n  x :: Num,\n  ` = () -> Text => \"P`it.x`\"\n}\n\
         describeIt = (p :: Point, f :: (Point) -> Text) -> Text => f(p)\n\
         ^ = () -> Num => describeIt(Point { x = 7 }, (p) => \"`p`!\").size",
        3,
    );
}

#[test]
fn a_member_call_types_its_lambda_from_the_receivers_member() {
    // A member call resolves against the receiver's type ALONE, so the lambda takes its
    // parameter from the method's `(Text) -> Num` — never from the top-level `applyTo`
    // that shares the name and states `(Num) -> Num`. 4 ("abcd") + 11 (10 + 1).
    assert_exit(
        "Calc = {\n  factor :: Num,\n  applyTo = (f :: (Text) -> Num) -> Num => f(\"abcd\")\n}\n\
         applyTo = (x :: Num, f :: (Num) -> Num) -> Num => f(x)\n\
         ^ = () -> Num => <\n  c = Calc { factor = 1 }\n  \
         c.applyTo((s) => s.size) + applyTo(10, (n) => n + 1)\n>",
        15,
    );
}

#[test]
fn an_unknown_member_on_a_lambda_receiver_names_the_receivers_type() {
    // A lambda receiver is left untyped by the dispatcher, and a member call resolves to no
    // signature that would type it — so the unknown-member report has to type it itself
    // rather than fall through to "undefined variable".
    let message = type_error_message("^ = () -> Num => ((n :: Num) => n + 1).foo(2)");
    assert!(
        message.contains("'(Num) -> Num' has no member 'foo'"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_lambda_handed_to_print_is_refused_as_unrenderable() {
    // `print` claims every one-argument call, and a lambda argument is left untyped by the
    // dispatcher — so the rendering rule has to type it before it can say what is wrong,
    // rather than mistaking the untyped argument for a miscounted one.
    let message = type_error_message("^ = () -> $ => print((n :: Num) => n + 1)");
    assert!(
        message.contains("renders its argument") && message.contains("(Num) -> Num has none"),
        "unexpected message: {message}"
    );
}

#[test]
fn overload_members_may_declare_their_signature_on_the_binding() {
    // An overload member written in the declared-type form keeps its full signature, so
    // exact dispatch still resolves it.
    assert_exit(
        "f :: (Num) -> Num = (n) => n + 1\n\
         f :: (Text) -> Num = (t) => t.size\n\
         ^ = () -> Num => f(1) + f(\"abc\")",
        5,
    );
}

#[test]
fn lambda_with_no_target_type_must_annotate() {
    // Nothing at this position states a function type — annotate, never a silent `Num`.
    let message = type_error_message("^ = () -> Num => <\n  fs = [(n) => n + 1]\n  fs.size\n>");
    assert!(
        message.contains("parameter 'n' of this lambda has no type"),
        "unexpected message: {message}"
    );
}

#[test]
fn overload_narrowed_by_its_other_arguments_types_the_lambda() {
    // The `Text`/`Num` first argument picks the member, and the member picked states what
    // the lambda's parameter is: `(Num) -> Num` for one, `(Text) -> Num` for the other.
    assert_exit(
        "run = (label :: Text, f :: (Num) -> Num) -> Num => f(1)\n\
         run = (label :: Num, f :: (Text) -> Num) -> Num => f(\"abc\")\n\
         ^ = () -> Num => run(\"x\", (n) => n + 1) + run(2, (t) => t.size)",
        5,
    );
}

#[test]
fn ambiguous_overload_requires_the_annotation() {
    // Nothing narrows the set to one member, so the target type is unknown and the error
    // says which set left it open.
    let message = type_error_message(
        "pick = (f :: (Num) -> Num) -> Num => f(1)\n\
         pick = (f :: (Text) -> Num) -> Num => f(\"a\")\n\
         ^ = () -> Num => pick((v) => 1)",
    );
    assert!(
        message.contains("parameter 'v' of this lambda has no type") && message.contains("'pick'"),
        "unexpected message: {message}"
    );
}

#[test]
fn ambiguous_overload_resolves_once_the_lambda_is_annotated() {
    assert_exit(
        "pick = (f :: (Num) -> Num) -> Num => f(1)\n\
         pick = (f :: (Text) -> Num) -> Num => f(\"abc\")\n\
         ^ = () -> Num => pick((t :: Text) -> Num => t.size)",
        3,
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
