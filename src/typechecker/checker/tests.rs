use super::*;
use crate::lexer::Lexer;
use crate::parser::parse;

#[test]
fn test_simple_var() {
    let tokens = Lexer::tokenize("x = 42").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_typed_var() {
    let tokens = Lexer::tokenize("x :: Num = 42").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_type_mismatch() {
    let tokens = Lexer::tokenize("x :: Text = 42").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
}

#[test]
fn test_arithmetic() {
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  result = 2 + 3 * 4\n  result\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_undefined_var() {
    let tokens = Lexer::tokenize("y = x + 1").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
}

#[test]
fn test_simple_function() {
    let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => < a + b >").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_function_call() {
    let tokens = Lexer::tokenize(
        "add = (a :: Num, b :: Num) -> Num => < a + b >
^ = () -> Num => <
  result = add(1, 2)
  result
>",
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_wrong_arg_count() {
    let tokens = Lexer::tokenize(
        "add = (a :: Num, b :: Num) -> Num => < a + b >
result = add(1)",
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
}

#[test]
fn test_array() {
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  nums = [1, 2, 3]\n  nums.size\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_array_type_mismatch() {
    let tokens = Lexer::tokenize("mixed = [1, \"hello\"]").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
}

#[test]
fn test_record() {
    let tokens = Lexer::tokenize(
        "^ = () -> Num => <\n  user = { name = \"Alice\", age = 30 }\n  user.age\n>",
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_if_expression() {
    let tokens =
        Lexer::tokenize("^ = () -> Num => <\n  result = true ? 1 : 0\n  result\n>").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_if_branch_type_mismatch() {
    let tokens = Lexer::tokenize("result = true ? 1 : \"hello\"").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
}

#[test]
fn test_block() {
    let tokens = Lexer::tokenize("compute = => < x = 10 y = 20 x + y >").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_pattern_match() {
    let tokens = Lexer::tokenize(
        "^ = () -> Text => <\n  result = 5 ? | 0 => \"zero\" | _ => \"other\"\n  result\n>",
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());
}

#[test]
fn test_inferred_return_type() {
    // Function without return type annotation - should infer from body
    let tokens = Lexer::tokenize("double = (x :: Num) => < x + x >").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_ok());

    // Verify the function type was inferred correctly
    let func_type = checker.env.get_type("double").unwrap();
    if let Type::Function {
        parameters,
        return_type,
    } = func_type
    {
        assert_eq!(parameters, vec![Type::Num]);
        assert_eq!(*return_type, Type::Num);
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_unannotated_parameter_is_rejected() {
    // A function parameter with no annotation cannot be inferred from context, so it is
    // a compile error rather than silently defaulting to Num.
    let tokens = Lexer::tokenize("add = (a, b) => < a + b >").unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    assert!(matches!(
        checker.check_program(&program),
        Err(TypeError::UnannotatedParameter { .. })
    ));
}

#[test]
fn test_sum_type_result_match() {
    // A `Result` scrutinee matched over both its variants.
    assert!(
        check_ok(
            "^ = () -> Num => <\n  val :: Result = Ok(5)\n  val ? | Ok(x) => x | NotOk(_) => 0\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_constructor_pattern_on_a_non_sum_scrutinee_is_rejected() {
    // A constructor pattern dispatches on a variant tag, which a `Num` has none of.
    assert!(matches!(
        check_ok("^ = () -> Num => <\n  val = 5\n  val ? | Ok(x) => x | _ => 0\n>"),
        Err(TypeError::ConstructorPatternOnNonSum { .. })
    ));
}

#[test]
fn test_unknown_constructor_is_rejected() {
    // `Maybe` is no variant of `Result`, and saying so here is what keeps codegen from
    // meeting a constructor it has no tag for.
    assert!(matches!(
        check_ok(
            "^ = () -> Num => <\n  val :: Result = Ok(5)\n  val ? | Maybe(x) => x | _ => 0\n>"
        ),
        Err(TypeError::UnknownConstructor { .. })
    ));
}

#[test]
fn test_exhaustiveness_with_wildcard() {
    // A wildcard covers the variants the listed arms don't.
    assert!(
        check_ok(
            "Color = Red / Green / Blue\n^ = () -> Num => <\n  c :: Color = Green\n  c ? | Red => 0 | _ => 1\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_non_exhaustive_match_on_a_non_sum_is_rejected() {
    // Nothing enumerates the values of a `Num`, so a match on one needs a `_` arm.
    assert!(matches!(
        check_ok("^ = () -> Num => <\n  val = 5\n  val ? | 0 => 1 | 1 => 2\n>"),
        Err(TypeError::NonExhaustiveMatch { .. })
    ));
}

#[test]
fn test_constructor_arity() {
    // A constructor pattern binds one sub-pattern per payload slot; `Ok` carries one.
    assert!(matches!(
        check_ok(
            "^ = () -> Num => <\n  val :: Result = Ok(5)\n  val ? | Ok(x, y) => x | NotOk(_) => 0\n>"
        ),
        Err(TypeError::WrongNumberOfArguments { .. })
    ));
}

#[test]
fn test_builtin_sum_types() {
    // Verify Result type is available
    let checker = TypeChecker::new();

    // Check Result is defined
    assert!(checker.env.get_type("Result").is_some());
}

fn check_ok(src: &str) -> Result<(), TypeError> {
    let tokens = Lexer::tokenize(src).unwrap();
    let program = parse(&tokens).unwrap();
    TypeChecker::new().check_program(&program).map(|_| ())
}

#[test]
fn test_overload_set_resolves_by_type() {
    // Two `f` definitions; each call resolves by exact argument type.
    assert!(
        check_ok(
            "f = (n :: Num) -> Num => < n >\nf = (s :: Text) -> Num => < s.size >\n^ = () -> Num => < f(1) + f(\"x\") >"
        )
        .is_ok()
    );
}

#[test]
fn test_overload_no_match_is_error() {
    // No `f` overload accepts a Bool (no implicit coercion).
    let err = check_ok(
        "f = (n :: Num) -> Num => < n >\nf = (s :: Text) -> Num => < s.size >\n^ = () -> Num => < f(true) >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::NoMatchingOverload { .. }));
}

#[test]
fn test_duplicate_overload_signature_is_error() {
    let err =
        check_ok("f = (n :: Num) -> Num => < n >\nf = (m :: Num) -> Num => < m >").unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_comparison_operator_overload_must_return_bool() {
    // A `==` member returning a non-Bool is rejected with a clear diagnostic.
    let err = check_ok("V = { x :: Num, == = (other :: V) -> V => < it >}\n^ = () -> Num => < 0 >")
        .unwrap_err();
    assert!(matches!(err, TypeError::ComparisonOverloadNotBool { .. }));
    // `<=` too (a definable comparison operator).
    assert!(
        check_ok("V = { x :: Num, <= = (other :: V) -> Num => < 1 >}\n^ = () -> Num => < 0 >")
            .is_err()
    );
}

#[test]
fn test_bool_returning_comparison_overload_is_accepted() {
    assert!(
        check_ok(
            "V = { x :: Num, == = (other :: V) -> Bool => < it.x == other.x >}\n^ = () -> Num => < V { x = 1 } == V { x = 1 } ? 1 : 0 >"
        )
        .is_ok()
    );
}

#[test]
fn test_arithmetic_operator_overload_return_type_is_unconstrained() {
    // No homogeneity rule on arithmetic operators: `V * Num -> V` is fine.
    assert!(
        check_ok(
            "V = { x :: Num, * = (k :: Num) -> V => < V { x = it.x } >}\n^ = () -> Num => <\n  w = V { x = 2 } * 3\n  w.x\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_user_operator_overload_typechecks() {
    assert!(
        check_ok(
            "P = { x :: Num, == = (other :: P) -> Bool => < it.x == other.x >}\n^ = () -> Num => < P { x = 1 } == P { x = 1 } ? 1 : 0 >"
        )
        .is_ok()
    );
}

#[test]
fn test_text_ordering_typechecks() {
    assert!(check_ok("^ = () -> Num => < \"a\" < \"b\" ? 1 : 0 >").is_ok());
    assert!(check_ok("^ = () -> Num => < \"a\" == \"a\" ? 1 : 0 >").is_ok());
}

#[test]
fn test_operator_no_overload_for_operands_is_error() {
    // `+` has no Num/Bool member.
    assert!(check_ok("^ = () -> Num => < 1 + true >").is_err());
}

#[test]
fn test_generic_payload_resolves_as_num_for_operators() {
    // A (generic) sum payload used with an operator resolves as Num — so
    // `Ok(x) => x * 2` type-checks against `*`'s (Num, Num) member. This pins the
    // documented Generic-as-Num overload behavior (concrete sum-payload typing is a
    // separate deferred feature); a future change here would be a visible regression.
    assert!(
        check_ok("^ = () -> Num => <\n  r = Ok(21)\n  r ? | Ok(x) => x * 2 | NotOk(e) => 0\n>")
            .is_ok()
    );
}

#[test]
fn test_ok_dispatch_over_builtin_payloads() {
    // Ok over Num/Text/Bool/$ all type-check.
    assert!(
        check_ok(
            "^ = () -> Num => <\n  a = Ok(1)\n  b = Ok(\"s\")\n  c = Ok(true)\n  d = Ok($)\n  0\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_for_loop_removed_is_rejected() {
    // The `for` loop was retired: iteration is via array methods / recursion.
    // A program using the old `for n <- collection => body` surface no longer
    // forms a loop — `for` is now an ordinary identifier — so it must fail to
    // compile (a parse or type error), never silently accept as before.
    let tokens = Lexer::tokenize("test = => < for n <- [1, 2, 3] => n >").unwrap();
    let compiles = match parse(&tokens) {
        Ok(program) => TypeChecker::new().check_program(&program).is_ok(),
        Err(_) => false, // rejected already at parse time
    };
    assert!(
        !compiles,
        "a `for` loop must no longer compile now that `for` is removed"
    );
}

#[test]
fn test_method_call_simple() {
    // Test that method calls work with type constructors
    let tokens = Lexer::tokenize(
        "User = {
  name :: Text,
  age :: Num,
  getName = => < it.name >
}
test = => <
  user = User { name = \"Alice\", age = 30 }
  name = user.getName()
  0
>",
    )
    .unwrap();
    let program = parse(&tokens).unwrap();
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);
    if let Err(e) = result.as_ref() {
        eprintln!("Type error: {:?}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_method_call_with_args() {
    // Test method calls with additional arguments
    check_ok(
        "Counter = {
  value :: Num,
  add = (x :: Num) -> Num => < it.value + x >
}
test = => <
  c = Counter { value = 5 }
  c.add(10)
>",
    )
    .unwrap();
}

#[test]
fn test_method_syntax_never_reaches_a_top_level_function() {
    // A top-level function is not a member of any type: `(5).double()` has no `double`
    // to resolve on `Num`, and the diagnostic names the type and the member.
    let err = check_ok(
        "double = (x :: Num) -> Num => < x * 2 >
^ = () -> Num => < (5).double() >",
    )
    .unwrap_err();
    assert!(
        matches!(
            &err,
            TypeError::UnknownMember { type_name, member, .. }
                if type_name == "Num" && member == "double"
        ),
        "expected UnknownMember for Num.double, got {err:?}"
    );
}

#[test]
fn test_overload_member_must_annotate_its_return_type() {
    // Called: the error lands on the call, which is where the unknown result type
    // stops the program, and names the member by its parameter types.
    let err = check_ok(
        "g = (n :: Num) => < \"a\" >\ng = (t :: Text) -> Text => < \"b\" >\nh = () -> Text => < g(1) >\n^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    match err {
        TypeError::UnannotatedOverloadCall {
            name, parameters, ..
        } => {
            assert_eq!(name, "g");
            assert_eq!(parameters, vec![Type::Num]);
        }
        other => panic!(
            "expected an unannotated-overload-call error, got {:?}",
            other
        ),
    }

    // Annotating it is all the fix takes.
    assert!(
        check_ok(
            "g = (n :: Num) -> Text => < \"a\" >\ng = (t :: Text) -> Text => < \"b\" >\nh = () -> Text => < g(1) >\n^ = () -> Num => < 0 >"
        )
        .is_ok()
    );
}

#[test]
fn test_uncalled_overload_member_missing_return_is_reported_at_its_definition() {
    // Nothing calls the unannotated member, so there is no call to blame — the
    // definition is reported instead, rather than the omission passing unnoticed.
    let err = check_ok(
        "g = (n :: Num) => < 1 >\ng = (t :: Text) -> Num => < 2 >\n^ = () -> Num => < g(\"x\") >",
    )
    .unwrap_err();
    match err {
        TypeError::UnannotatedOverloadMember {
            name, parameters, ..
        } => {
            assert_eq!(name, "g");
            assert_eq!(parameters, vec![Type::Num]);
        }
        other => panic!(
            "expected an unannotated-overload-member error, got {:?}",
            other
        ),
    }
}

#[test]
fn test_unannotated_overload_member_return_is_never_inferred_from_its_body() {
    // A body that plainly returns Text does not excuse the annotation, and checking
    // that body first does not rescue a later call: inferring the member's return
    // would make its signature depend on where the call sits relative to the
    // definition, which is the order dependence the requirement removes.
    let err = check_ok(
        "g = (n :: Num) => < \"a\" >\ng = (t :: Text) -> Text => < \"b\" >\n^ = () -> Num => < g(1).size >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::UnannotatedOverloadCall { .. }));
}

#[test]
fn test_overload_member_recursion_needs_the_annotation_then_works() {
    // A member calling itself hits the same rule (its own return type is what the
    // recursive call needs)…
    assert!(
        check_ok(
            "p = (n :: Num) => < n == 0 ? \"done\" : p(n - 1) >\np = (t :: Text) -> Num => < 0 >\n^ = () -> Num => < 0 >"
        )
        .is_err()
    );
    // …and annotating it makes the recursive member legal.
    assert!(
        check_ok(
            "p = (n :: Num) -> Text => < n == 0 ? \"done\" : p(n - 1) >\np = (t :: Text) -> Num => < 0 >\n^ = () -> Num => < p(3).size >"
        )
        .is_ok()
    );
}

#[test]
fn test_call_to_an_overload_member_defined_below_is_rejected() {
    // Members join their set where they are written, so this call sees no `g` at all.
    // It used to resolve against the pre-registered signature and then fail in
    // codegen with no matching symbol.
    let err = check_ok(
        "h = () -> Text => < g(1) >\ng = (n :: Num) -> Text => < \"a\" >\ng = (t :: Text) -> Text => < \"b\" >\n^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    match err {
        TypeError::OverloadCallBeforeDefinition { name, .. } => assert_eq!(name, "g"),
        other => panic!("expected a call-before-definition error, got {:?}", other),
    }
}

#[test]
fn test_mutually_recursive_overload_members_are_rejected_not_miscompiled() {
    // Whichever of the pair comes first must call the other before it exists. This
    // type-checked before and died in codegen; now it is refused at the forward call.
    let err = check_ok(
        "even = (n :: Num) -> Bool => < n == 0 ? true : odd(n - 1) >\neven = (t :: Text) -> Bool => < false >\nodd = (n :: Num) -> Bool => < n == 0 ? false : even(n - 1) >\nodd = (t :: Text) -> Bool => < true >\n^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    match err {
        TypeError::OverloadCallBeforeDefinition { name, .. } => assert_eq!(name, "odd"),
        other => panic!("expected a call-before-definition error, got {:?}", other),
    }
}

#[test]
fn test_a_call_resolves_against_the_members_above_it() {
    // Only `f`'s Num member is defined at the call, so a Text argument reports the
    // candidates that actually exist there rather than reaching forward.
    let err = check_ok(
        "f = (n :: Num) -> Num => < 1 >\nh = () -> Num => < f(\"x\") >\nf = (t :: Text) -> Num => < 2 >\n^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    match err {
        TypeError::NoMatchingOverload { candidates, .. } => {
            assert_eq!(candidates, vec![vec![Type::Num]]);
        }
        other => panic!("expected a no-matching-overload error, got {:?}", other),
    }
}

#[test]
fn test_unannotated_comparison_operator_overload_asks_for_the_annotation() {
    // Without a return type there is nothing to compare against `Bool` yet, so the
    // actionable message wins: annotate it. Annotated non-Bool still gets the
    // comparison-specific error.
    let err = check_ok("V = { x :: Num, == = (other :: V) => < it >}\n^ = () -> Num => < 0 >")
        .unwrap_err();
    assert!(matches!(err, TypeError::UnannotatedOverloadMember { .. }));
}

#[test]
fn test_named_update_accepts_its_own_type_or_that_exact_shape() {
    // The source is already the type being built.
    assert!(
        check_ok(
            "P = { x :: Num, y :: Num }\n^ = () -> Num => <\n  a = P { x = 1, y = 2 }\n  b = P { <-a, x = 9 }\n  b.x\n>"
        )
        .is_ok()
    );
    // An anonymous record of exactly that shape fills a type with no methods.
    assert!(
        check_ok(
            "P = { x :: Num, y :: Num }\n^ = () -> Num => <\n  parts = { x = 1, y = 2 }\n  b = P { <-parts }\n  b.x\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_named_update_refuses_a_source_that_cannot_fill_the_type() {
    // A different named type is not interchangeable, however alike its fields.
    let other = check_ok(
        "P = { x :: Num }\nQ = { x :: Num }\n^ = () -> Num => <\n  q = Q { x = 1 }\n  p = P { <-q }\n  p.x\n>",
    )
    .unwrap_err();
    assert!(matches!(other, TypeError::TypeMismatch { .. }));

    // An anonymous record carries no methods, so it cannot fill a type that has one.
    let with_method = check_ok(
        "V = { x :: Num, double = => < it.x * 2 >}\n^ = () -> Num => <\n  parts = { x = 1 }\n  v = V { <-parts }\n  v.x\n>",
    )
    .unwrap_err();
    assert!(matches!(with_method, TypeError::TypeMismatch { .. }));

    // A shape that is missing one of the declared fields cannot fill it either.
    let short = check_ok(
        "P = { x :: Num, y :: Num }\n^ = () -> Num => <\n  parts = { x = 1 }\n  p = P { <-parts }\n  p.x\n>",
    )
    .unwrap_err();
    assert!(matches!(short, TypeError::TypeMismatch { .. }));
}
