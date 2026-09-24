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
fn test_result_parameter_pinned_from_a_direct_call() {
    // `classify`'s parameter has no payload type of its own — every bare `:: Result`
    // annotation is the same unspecialized shape — but its one direct caller passes a
    // concrete `Ok(Text)` argument, and the checker pins `text` to that real type
    // rather than leaving it generic (see `sums::pin_result_parameters`).
    assert!(
        check_ok(
            "classify = (result :: Result) -> Text => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => <\n  \
               a = classify(Ok(\"hi\"))\n  \
               a.length\n\
             >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_disagreeing_callers_is_a_type_mismatch() {
    // A second caller passing a different concrete type for a position the first
    // caller already pinned is a `TypeMismatch` at that second call, the same rule a
    // constructor's own argument already enforces.
    assert!(matches!(
        check_ok(
            "classify = (result :: Result) -> Text => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => <\n  \
               a = classify(Ok(\"hi\"))\n  \
               b = classify(Ok(5))\n  \
               0\n\
             >"
        ),
        Err(TypeError::TypeMismatch { .. })
    ));
}

#[test]
fn test_result_parameter_of_a_function_unreachable_from_the_entry_point_is_not_checked() {
    // `classify` is called nowhere reachable from `^` at all — codegen's own
    // `reachable_functions` prunes it, so it is never emitted, and this pass skips its
    // payload entirely rather than reporting a shape nothing will ever need a
    // representation for. The program is still rejected — a non-exported function
    // nothing calls is dead code (`NeverReachable`) — but the skip means THAT is the
    // error raised, not a spurious `UnresolvedResultPayload` for a payload the dead
    // function never demonstrates.
    let err = check_ok(
        "classify = (result :: Result) -> Text => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    assert!(
        matches!(err, TypeError::NeverReachable { ref name, .. } if name == "classify"),
        "expected `classify` reported as dead code, not a Result-payload error: {err:?}"
    );
}

#[test]
fn test_result_parameter_bound_and_read_but_never_demonstrated_by_a_caller_is_rejected() {
    // `judge` is called only with `Ok(...)`, so its `NotOk` position is never
    // demonstrated by any direct caller — and its own binding is READ (passed on to
    // `describe`), so nothing says what its real type is. The specific thing the
    // binding is used FOR doesn't matter under this rule (a constructor argument, a
    // plain call, a record field, arithmetic, …) — any read of an undemonstrated
    // payload is rejected uniformly.
    assert!(matches!(
        check_ok(
            "describe = (text :: Text) -> Text => < \"got: \" + text >\n\
             judge = (result :: Result) -> Text => <\n  \
               result ?\n    \
                 | Ok(text)     => text\n    \
                 | NotOk(text2) => describe(text2)\n\
             >\n\
             ^ = () -> Num => < judge(Ok(\"hi\")).length >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
}

#[test]
fn test_result_parameter_bound_but_never_read_is_accepted_even_when_never_demonstrated() {
    // `x` is bound (`Ok(x)`, not `Ok(_)`) but never READ anywhere in its own arm — the
    // arm's whole body is the literal `0`, which never touches `x` — so no caller
    // needs to demonstrate `Ok`'s payload type at all. Binding a name is not itself a
    // read; only a later USE of it is.
    assert!(
        check_ok(
            "classify = (result :: Result) -> Num => <\n  \
               result ? | Ok(x) => 0 | NotOk(_) => 1\n\
             >\n\
             ^ = () -> Num => < classify(NotOk(\"unused\")) >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_shadow_of_an_unrelated_sibling_local_does_not_hide_a_real_match() {
    // Regression: two SIBLING nested functions each declare their own, unrelated local
    // named `copy`; one of them happens to copy the outer `result` alias into its OWN
    // `copy`, the other rebinds ITS OWN `copy` to something else entirely. A shadow
    // keyed only by BYTE RANGE (not by name) would wrongly treat the second's rebind as
    // shadowing the first's alias, hiding a genuine match on `result` inside the
    // SECOND function and letting an unresolved payload through uncaught. `outer` is
    // called from `^` with `NotOk`, so it is reachable (this pass still runs on it) but
    // `Ok` is still never demonstrated.
    assert!(matches!(
        check_ok(
            "outer = (result :: Result) -> Text => <\n  \
               branchA = () -> Num => <\n    \
                 copy = result\n    \
                 0\n  \
               >\n  \
               branchA()\n  \
               branchB = () -> Text => <\n    \
                 copy = Ok(5)\n    \
                 result ? | Ok(text) => text | NotOk(_) => \"none\"\n  \
               >\n  \
               branchB()\n\
             >\n\
             ^ = () -> Num => < outer(NotOk(\"x\")).length >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
}

#[test]
fn test_result_parameter_of_a_method_is_pinned_from_its_call_site() {
    // `check_call` resolves a member call to its receiver's type — `Box`'s own
    // `unwrap` here — so `sums::pin_result_parameters` pins `text` from THAT call's
    // own `Ok("hi")` argument the same way a plain function's direct caller already
    // does, rather than rejecting every method read unconditionally.
    assert!(
        check_ok(
            "Box = {\n  \
               value :: Num,\n  \
               unwrap = (result :: Result) -> Text => <\n    \
                 result ? | Ok(text) => text | NotOk(_) => \"none\"\n  \
               >\n\
             }\n\
             ^ = () -> Num => <\n  \
               b = Box { value = 1 }\n  \
               b.unwrap(Ok(\"hi\")).length\n\
             >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_method_never_passed_ok_is_rejected() {
    // The SAME method, but every call passes `NotOk` — `Ok`'s payload is never
    // demonstrated by any caller, so reading `text` (bound from `Ok(text)`) is still
    // `UnresolvedResultPayload`, exactly like an ungathered plain function parameter.
    assert!(matches!(
        check_ok(
            "Box = {\n  \
               value :: Num,\n  \
               unwrap = (result :: Result) -> Text => <\n    \
                 result ? | Ok(text) => text | NotOk(_) => \"none\"\n  \
               >\n\
             }\n\
             ^ = () -> Num => <\n  \
               b = Box { value = 1 }\n  \
               b.unwrap(NotOk(\"lost\")).length\n\
             >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
}

#[test]
fn test_result_parameter_of_a_method_bound_but_unread_is_accepted() {
    // The SAME method as above, but its `Ok` binding is never read (the arm's whole
    // body is a literal) — a method's parameter has no pin at all just like the read
    // case, but with nothing reading the payload, no type is needed either.
    assert!(
        check_ok(
            "Box = {\n  \
               value :: Num,\n  \
               unwrap = (result :: Result) -> Num => <\n    \
                 result ? | Ok(text) => 0 | NotOk(_) => 1\n  \
               >\n\
             }\n\
             ^ = () -> Num => <\n  \
               b = Box { value = 1 }\n  \
               b.unwrap(Ok(\"hi\"))\n\
             >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_an_overloaded_function_is_pinned_from_its_call_site() {
    // `resolve_overload` resolves a bare `describe(Ok("home"))` call to exactly one
    // member by its argument types — the one-argument, `Result`-taking `describe` —
    // so `sums::pin_result_parameters` pins `text` from THAT call's own argument the
    // same way a plain (non-overloaded) function's direct caller already does.
    assert!(
        check_ok(
            "describe = (result :: Result) -> Text => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             describe = (n :: Num) -> Text => < \"num\" >\n\
             ^ = () -> Num => < describe(Ok(\"hi\")).length >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_an_overloaded_function_never_passed_ok_is_rejected() {
    // A bare call to an overloaded name doesn't inform every member equally — this
    // one-argument `handle` is only ever reached FORWARDED THROUGH the two-argument
    // member (`handle(result)`, an identifier, not a constructor call), so its own
    // `Ok` is never directly demonstrated — a READ of its bound payload is still
    // `UnresolvedResultPayload`.
    assert!(matches!(
        check_ok(
            "handle = (result :: Result) -> Text => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             handle = (result :: Result, prefix :: Text) -> Text => < prefix + handle(result) >\n\
             ^ = () -> Num => < handle(Ok(\"hi\"), \">\").length >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
}

#[test]
fn test_result_parameter_only_self_recursive_with_a_read_binding_is_rejected() {
    // `loopy` is called from `^` (so it is reachable, and this pass runs on it), but
    // only with `NotOk` — its OWN self-recursive call forwards `result` rather than
    // passing a concrete `Ok`/`NotOk` argument, so `Ok`'s payload is never
    // demonstrated by ANY caller, direct or indirect. Since `t` IS read (it's the
    // whole `Ok` arm's body), this is rejected: self-recursion is not a caller that
    // can teach this pass anything.
    assert!(matches!(
        check_ok(
            "loopy = (result :: Result) -> Text => <\n  \
               result ? | Ok(t) => t | NotOk(_) => loopy(result)\n\
             >\n\
             ^ = () -> Num => < loopy(NotOk(\"x\")).length >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
}

#[test]
fn test_result_parameter_only_matched_with_a_wildcard_is_accepted() {
    // Neither variant is ever BOUND (`Ok(_)`/`NotOk(_)`), so there is no payload type to
    // resolve at all — a function dispatched on the tag alone, every call passing a
    // different concrete payload, is untouched by this rule.
    assert!(
        check_ok(
            "okTag = (r :: Result) -> Num => < r ? | Ok(_) => 1 | NotOk(_) => 0 >\n\
             ^ = () -> Num => <\n  \
               a = okTag(Ok(42))\n  \
               b = okTag(Ok(\"hi\"))\n  \
               c = okTag(NotOk(7))\n  \
               a + b + c\n\
             >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_shadowed_by_a_local_reassignment_is_not_falsely_rejected() {
    // `helper`'s own local `result := Ok(x)` reuses `outer`'s parameter name, but is a
    // fresh, already-concrete (`Num`) binding — the checker's own first-pass record of
    // its scrutinee's real type (not just its name) keeps this from being mistaken for
    // a read of `outer`'s own (unused, here) parameter, which would otherwise falsely
    // reject a program that never actually needs this rule at all.
    assert!(
        check_ok(
            "outer = (result :: Result) -> Num => <\n  \
               helper = (x :: Num) -> Num => <\n    \
                 result = Ok(x)\n    \
                 result ? | Ok(n) => n | NotOk(_) => 0\n  \
               >\n  \
               helper(5)\n\
             >\n\
             ^ = () -> Num => < outer(Ok(\"hi\")) >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_bound_lambda_is_pinned_too() {
    // A `:=`-bound lambda's bare `:: Result` parameter is pinned from its direct
    // caller exactly like a `FunctionDeclaration`'s — this pass visits every
    // function-shaped declaration in the program uniformly, including one bound this
    // way, and it is never overloaded (only a `FunctionDeclaration`'s name joins
    // `overloaded_names`).
    assert!(
        check_ok(
            "judge := (act :: Result) => < act ? | Ok(text) => text | NotOk(_) => \"n\" >\n\
             ^ = () -> Num => < judge(Ok(\"hi\")).length >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_never_matched_is_not_falsely_rejected_by_a_nested_shadow() {
    // The method's own `result` parameter is never itself matched — only a NESTED
    // helper's unrelated local `result := Ok(x)`, reusing the name, is. A false
    // `UnresolvedResultPayload` here would mean the method's parameter was mistaken for
    // that unrelated, already-concrete local.
    assert!(
        check_ok(
            "Box = {\n  \
               value :: Num,\n  \
               unwrap = (result :: Result) -> Num => <\n    \
                 helper = (x :: Num) -> Num => <\n      \
                   result = Ok(x)\n      \
                   result ? | Ok(n) => n | NotOk(_) => 0\n    \
                 >\n    \
                 helper(it.value)\n  \
               >\n\
             }\n\
             ^ = () -> Num => <\n  \
               b = Box { value = 5 }\n  \
               b.unwrap(Ok(\"unused\"))\n\
             >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_nested_function_is_pinned_too() {
    // `classify` is declared INSIDE `outer`'s body, not at the top level — this pass
    // still visits it, at whatever depth, and pins its own bound `:: Result` parameter
    // from its direct caller exactly like a top-level function's.
    assert!(
        check_ok(
            "outer = () -> Num => <\n  \
               classify = (result :: Result) -> Text => <\n    \
                 result ? | Ok(text) => text | NotOk(_) => \"none\"\n  \
               >\n  \
               classify(Ok(\"hello world\")).length\n\
             >\n\
             ^ = () -> Num => < outer() >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_via_a_whole_signature_annotation_is_pinned() {
    // `classify`'s parameter has no annotation of its OWN — its type comes from the
    // binding's whole-signature `:: (Result) -> Text` form instead
    // (`FunctionDeclaration::declared_parameters`). This pass must resolve a
    // parameter's type the same way the checker's own `resolve_parameter_types` does,
    // not just read `type_annotation` directly, or this form's bound payload never
    // gets a chance to pin.
    assert!(
        check_ok(
            "classify :: (Result) -> Text = (result) => <\n  \
               result ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => < classify(Ok(\"hi\")).length >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_alias_chain_of_several_hops_is_still_pinned() {
    // `a`, `b`, and `c` are each a direct copy of the previous, chasing back to
    // `result` — the alias set grows within one walk, top to bottom, so a chain isn't
    // just a single rename away from escaping detection, and the match on `c` still
    // pins `result`'s own payload from `classify`'s one direct caller.
    assert!(
        check_ok(
            "classify = (result :: Result) -> Text => <\n  \
               a = result\n  \
               b = a\n  \
               c = b\n  \
               c ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => < classify(Ok(\"hi\")).length >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_nested_whole_signature_declaration_is_pinned() {
    // `classify`'s whole-signature `:: (Result) -> Text` form works the same way
    // whether it's declared at the top level or, as here, inside another function's
    // body — `nested_function_candidates` must carry a nested `FunctionDeclaration`'s
    // `declared_parameters()` through exactly like a top-level one's.
    assert!(
        check_ok(
            "outer = () -> Num => <\n  \
               classify :: (Result) -> Text = (result) => <\n    \
                 result ? | Ok(text) => text | NotOk(_) => \"none\"\n  \
               >\n  \
               classify(Ok(\"hi\")).length\n\
             >\n\
             ^ = () -> Num => < outer() >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_renamed_before_matching_is_still_pinned() {
    // `renamed` is a direct copy of `result` (`renamed = result`, no transformation) —
    // matching it must still be attributed back to `result`'s own parameter, pinning
    // its payload from `classify`'s direct caller, not missed because the match reads
    // a different identifier.
    assert!(
        check_ok(
            "classify = (result :: Result) -> Text => <\n  \
               renamed = result\n  \
               renamed ? | Ok(text) => text | NotOk(_) => \"none\"\n\
             >\n\
             ^ = () -> Num => < classify(Ok(\"hi\")).length >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_never_referenced_nested_bound_lambda_is_named_correctly() {
    // `helper` is a `:=`-bound lambda declared INSIDE `outer`'s body, called nowhere at
    // all — the diagnostic must still name it `helper`, not the generic "a lambda" an
    // anonymous callback would get.
    assert!(matches!(
        check_ok(
            "outer = () -> Num => <\n  \
               helper := (result :: Result) => < result ? | Ok(x) => x | NotOk(_) => 0 >\n  \
               0\n\
             >\n\
             ^ = () -> Num => < outer() >"
        ),
        Err(TypeError::UnresolvedResultPayload { function, .. })
            if function == "helper"
    ));
}

#[test]
fn test_result_parameter_of_a_locally_declared_types_method_is_pinned_from_its_call_site() {
    // `Box` is declared INSIDE `outer`'s body, not at the program's top level —
    // `unwrap`'s bare `:: Result` parameter is still pinned from its own call site,
    // exactly like a top-level type's method's is.
    assert!(
        check_ok(
            "outer = () -> Num => <\n  \
               Box = {\n    \
                 value :: Num,\n    \
                 unwrap = (result :: Result) -> Text => <\n      \
                   result ? | Ok(text) => text | NotOk(_) => \"none\"\n    \
                 >\n  \
               }\n  \
               b = Box { value = 1 }\n  \
               b.unwrap(Ok(\"hi\")).length\n\
             >\n\
             ^ = () -> Num => < outer() >"
        )
        .is_ok()
    );
}

#[test]
fn test_result_parameter_of_a_locally_declared_types_method_never_passed_ok_is_rejected() {
    // The SAME locally-declared method, but every call passes `NotOk` — `Ok`'s
    // payload is never demonstrated, so reading `text` is still `UnresolvedResultPayload`.
    assert!(matches!(
        check_ok(
            "outer = () -> Num => <\n  \
               Box = {\n    \
                 value :: Num,\n    \
                 unwrap = (result :: Result) -> Text => <\n      \
                   result ? | Ok(text) => text | NotOk(_) => \"none\"\n    \
                 >\n  \
               }\n  \
               b = Box { value = 1 }\n  \
               b.unwrap(NotOk(\"lost\")).length\n\
             >\n\
             ^ = () -> Num => < outer() >"
        ),
        Err(TypeError::UnresolvedResultPayload { .. })
    ));
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
fn test_text_literal_pattern_with_catch_all_is_accepted() {
    // A `Text` scrutinee matched against literal text, covered by a trailing catch-all.
    assert!(
        check_ok(
            "^ = () -> Num => <\n  val = \"GET\"\n  val ? | \"GET\" => 0 | \"POST\" => 1 | _ => 2\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_text_literal_pattern_without_catch_all_is_rejected() {
    // Nothing enumerates the values of a `Text` either — the same rule a `Num` match
    // follows.
    assert!(matches!(
        check_ok("^ = () -> Num => <\n  val = \"GET\"\n  val ? | \"GET\" => 0 | \"POST\" => 1\n>"),
        Err(TypeError::NonExhaustiveMatch { .. })
    ));
}

#[test]
fn test_text_literal_pattern_against_a_num_scrutinee_is_rejected() {
    // The same mismatch a `Number` pattern gives against a `Text` scrutinee, the other
    // way round.
    assert!(matches!(
        check_ok("^ = () -> Num => <\n  val = 5\n  val ? | \"GET\" => 0 | _ => 1\n>"),
        Err(TypeError::TypeMismatch { .. })
    ));
}

#[test]
fn test_text_literal_pattern_against_a_sum_is_rejected() {
    // A sum's values carry no `Text`, so a text pattern can't dispatch on one.
    assert!(matches!(
        check_ok("^ = () -> Num => <\n  val :: Result = Ok(5)\n  val ? | \"GET\" => 0 | _ => 1\n>"),
        Err(TypeError::TypeMismatch { .. })
    ));
}

#[test]
fn test_constructor_missing_field_is_rejected() {
    // `P { x = 1 }` leaves out `P`'s declared `y` field.
    assert!(matches!(
        check_ok("P = { x :: Num, y :: Num }\n^ = () -> Num => < p = P { x = 1 }  0 >"),
        Err(TypeError::MissingConstructorField { .. })
    ));
}

#[test]
fn test_constructor_unknown_field_is_rejected() {
    // `P` declares no `z` field.
    assert!(matches!(
        check_ok(
            "P = { x :: Num, y :: Num }\n^ = () -> Num => < p = P { x = 1, y = 2, z = 3 }  0 >"
        ),
        Err(TypeError::UnknownConstructorField { .. })
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
    let checker = TypeChecker::new();
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
fn test_overloaded_members_returning_result_refine_independently() {
    // Two `pick` overloads, each declared the one generic annotation the language can
    // write (`-> Result`), with a DIFFERENT concrete `Ok` payload — one call sits
    // textually BETWEEN the two declarations, one AFTER both. If refinement were shared
    // or order-dependent across the set, one of these would see the other member's
    // payload (or a still-generic one) instead of its own.
    let src = "\
pick = (flag :: Bool) -> Result => < Ok(\"hello\") >

between = () -> Text => <
  pick(true) ? | Ok(t) => t | NotOk(_) => \"\"
>

pick = (flag :: Num) -> Result => < Ok(true) >

after = () -> Bool => <
  pick(5) ? | Ok(b) => b | NotOk(_) => false
>

^ = () -> Num => <
  between()
  after()
  0
>
";
    let tokens = Lexer::tokenize(src).unwrap();
    let program = parse(&tokens).unwrap();
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("both overload members should refine their own Result payload independently");

    let has_ok_payload = |payload: &Type| {
        types.values().any(|recorded| {
            matches!(recorded, Type::Sum { name, variants } if name == "Result"
                && variants
                    .iter()
                    .any(|variant| variant.name == "Ok" && variant.fields == vec![payload.clone()]))
        })
    };
    assert!(
        has_ok_payload(&Type::Text),
        "the Bool-argument member's call (between the declarations) must see its own Ok(Text)"
    );
    assert!(
        has_ok_payload(&Type::Bool),
        "the Num-argument member's call (after both declarations) must see its own Ok(Bool)"
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

fn reserved(src: &str) -> bool {
    matches!(check_ok(src), Err(TypeError::ReservedName { .. }))
}

#[test]
fn test_a_reserved_name_is_refused_at_every_binding_kind() {
    // A type declaration, a `=` binding, a `:=` binding, a function, a parameter, a lambda
    // parameter, a pattern binding, and an overload member — a mix of the reserved words.
    assert!(reserved("Map = A / B"));
    assert!(reserved("^ = () -> Num => < it = 1  it >"));
    assert!(reserved("^ = () -> Num => < not := 1  not >"));
    assert!(reserved(
        "contains = (t :: Text) -> Bool => < true >\n^ = () -> Num => < 0 >"
    ));
    assert!(reserved(
        "f = (equals :: Num) -> Num => < equals >\n^ = () -> Num => < f(1) >"
    ));
    assert!(reserved(
        "^ = () -> Num => < [1].map(expect => expect)[0] >"
    ));
    assert!(reserved(
        "^ = () -> Num => < r :: Result = Ok(1)  r ? | Ok(isOk) => isOk | NotOk(_) => 0 >"
    ));
    assert!(reserved(
        "assert = (n :: Num) -> Num => < n >\nassert = (t :: Text) -> Num => < 0 >\n\
         ^ = () -> Num => < 0 >"
    ));
}

#[test]
fn test_a_member_may_carry_a_reserved_name() {
    // A field or method is not a binding: `it`, `contains`, and `not` are all legal member
    // names, and reached as members.
    assert!(
        check_ok(
            "T = { it :: Num, contains = (n :: Num) -> Bool => < it.it == n >, not = () -> Num => < 0 - it.it > }\n\
             ^ = () -> Num => <\n  t = T { it = 3 }\n  (t.contains(3) ? 1 : 0) + t.not()\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_assertions_and_matchers_still_work_as_calls() {
    assert!(
        check_ok(
            "^ = () -> Num => <\n  assert(1, equals(1))\n  assert(\"ab\", contains(\"a\"))\n  assert(1, not(equals(2)))\n  r :: Result = Ok(1)\n  assert(r, isOk())\n  0\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_constructor_literal_duplicate_field_is_rejected() {
    // A SECOND literal `x = 3` names a field the constructor already provided.
    let err = check_ok(
        "P = { x :: Num, y :: Num }\n^ = () -> Num => < p = P { x = 1, y = 2, x = 3 }  0 >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_anonymous_record_literal_duplicate_field_is_rejected() {
    let err = check_ok("^ = () -> Num => < q = { x = 1, x = 2 }  0 >").unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_duplicate_declared_field_is_rejected() {
    // `T` declares `a` twice, once as each type — the second declaration collides
    // regardless of its own type.
    let err = check_ok("T = { a :: Num, a :: Text }\n^ = () -> Num => < 0 >").unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_field_and_method_sharing_a_name_is_accepted() {
    // A bare access reaches the field, a dot-call reaches the method — both type-check.
    assert!(
        check_ok(
            "T = { a :: Num, a = () -> Num => < it.a + 1 > }\n\
             ^ = () -> Num => <\n  t = T { a = 9 }\n  t.a + t.a()\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_field_and_static_method_sharing_a_name_is_accepted() {
    // The static method is reached on the bare type name; the field, on a value.
    assert!(
        check_ok(
            "T = { a :: Num, a = (n :: Num) -> T => < T { a = n } > }\n\
             ^ = () -> Num => <\n  t = T.a(9)\n  t.a\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_function_typed_field_is_rejected() {
    let err = check_ok("Box = { scale :: (Num) -> Num }\n^ = () -> Num => < 0 >").unwrap_err();
    assert!(matches!(err, TypeError::FunctionTypedField { .. }));
}

#[test]
fn test_function_typed_method_parameter_still_works() {
    // Function-typed parameters, bindings, and return types are unaffected by the field
    // restriction — only a FIELD may not carry a function type.
    assert!(
        check_ok(
            "Box = { value :: Num, apply = (f :: (Num) -> Num) -> Num => < f(it.value) > }\n\
             ^ = () -> Num => <\n  b = Box { value = 4 }\n  b.apply(n => n * 2)\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_duplicate_same_signature_method_in_a_record_is_rejected() {
    let err = check_ok(
        "T = { a :: Num, f = () -> Num => < 1 >, f = () -> Num => < 2 > }\n\
         ^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_duplicate_same_signature_method_in_a_sum_is_rejected() {
    let err = check_ok(
        "S = A / B { f = () -> Num => < 1 >, f = () -> Num => < 2 > }\n\
         ^ = () -> Num => < 0 >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::DuplicateDefinition { .. }));
}

#[test]
fn test_method_overload_set_resolves_by_type() {
    // Two `f` methods on the same type, differing only in their parameter's type — a
    // legitimate overload set, dispatched by exact argument type exactly like a
    // top-level overload.
    assert!(
        check_ok(
            "T = { a :: Num, f = (n :: Num) -> Num => < n >, f = (s :: Text) -> Num => < s.size > }\n\
             ^ = () -> Num => < t = T { a = 1 }  t.f(1) + t.f(\"xy\") >"
        )
        .is_ok()
    );
}

#[test]
fn test_static_overload_set_resolves_by_type() {
    // Two `make` methods differing only in their parameter's type, neither reading
    // `it` — a static overload set dispatches on the bare type name exactly as a
    // value-receiver overload dispatches on a value, resolving each call to its own
    // member by argument type.
    assert!(
        check_ok(
            "T = { a :: Num, make = (n :: Num) -> Num => < n >, make = (s :: Text) -> Num => < s.size > }\n\
             ^ = () -> Num => < T.make(1) + T.make(\"xy\") >"
        )
        .is_ok()
    );
}

#[test]
fn test_static_overload_set_resolves_by_arity() {
    // Two `make` methods differing only in arity, neither reading `it`.
    assert!(
        check_ok(
            "T = { a :: Num, make = (n :: Num) -> Num => < n >, make = (n :: Num, m :: Num) -> Num => < n + m > }\n\
             ^ = () -> Num => < T.make(1) + T.make(1, 2) >"
        )
        .is_ok()
    );
}

#[test]
fn test_overload_member_that_reads_it_still_needs_a_receiver_value() {
    // An overload set still requires a receiver VALUE for the specific member that
    // reads `it` — a same-named static sibling does not exempt it.
    assert!(matches!(
        check_ok(
            "T = { a :: Num, f = (n :: Num) -> Num => < n >, f = (s :: Text) -> Num => < it.a > }\n\
             ^ = () -> Num => < T.f(\"xy\") >"
        ),
        Err(TypeError::StaticCallNeedsReceiverValue { .. })
    ));
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
    // Iteration is via array methods / recursion; `for` is an ordinary identifier, not a
    // keyword, so a `for n <- collection => body` surface does not form a loop and must
    // fail to compile (a parse or type error), never silently accept.
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
            "g = (n :: Num) -> Text => < \"a\" >\ng = (t :: Text) -> Text => < \"b\" >\nh = () -> Text => < g(1) >\n^ = () -> Num => <\n  h()\n  0\n>"
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

#[test]
fn test_atomic_binding_typechecks_like_a_plain_mutable_one() {
    // `@name := …` accepts any type, and behaves exactly like `:=` on the single-threaded
    // runtime — declared, reassigned bare, and read bare.
    assert!(
        check_ok(
            "@hits := 0\n\
             bump = () -> Num => < hits := hits + 1\n  hits >\n\
             ^ = () -> Num => < bump()\n  bump()\n  hits >"
        )
        .is_ok()
    );
    // A block-local atomic binding of a record value.
    assert!(
        check_ok(
            "Stand = { open :: Bool }\n\
             ^ = () -> Bool => <\n  @stand := Stand { open = true }\n  stand.open\n>"
        )
        .is_ok()
    );
}

#[test]
fn test_atomic_binding_without_mutable_is_rejected() {
    // `@name = value` — atomic makes sense only for a mutable binding.
    let err = check_ok("@hits = 0\n^ = () -> Num => < hits >").unwrap_err();
    assert!(matches!(err, TypeError::AtomicBindingNotMutable { .. }));
}

#[test]
fn test_atomic_binding_reassigned_with_at_marker_is_rejected() {
    // `@` marks the declaration only; a reassignment stays bare.
    let err =
        check_ok("^ = () -> Num => <\n  @hits := 0\n  @hits := hits + 1\n  hits\n>").unwrap_err();
    assert!(matches!(err, TypeError::AtomicBindingUsedBare { .. }));
}

#[test]
fn test_atomic_binding_read_with_at_marker_is_rejected() {
    // `@name` at a use site, once `name` is an atomic binding, names the same mistake as a
    // `@`-marked reassignment rather than the generic "undefined name" a stray `@`-prefixed
    // primitive reference would otherwise get.
    let err = check_ok("@hits := 0\n^ = () -> Num => < @hits >").unwrap_err();
    assert!(matches!(err, TypeError::AtomicBindingUsedBare { .. }));
}

#[test]
fn test_an_unrelated_at_prefixed_name_still_reports_undefined() {
    // No binding named `bogus` exists, atomic or otherwise, so a stray `@bogus` keeps the
    // ordinary "undefined name" report.
    let err = check_ok("^ = () -> Num => < @bogus() >").unwrap_err();
    assert!(matches!(err, TypeError::UndefinedVariable { .. }));
}

/// Lex, parse, and link `src` (real corelib modules, since `net.@tcpServe` only exists
/// through `<< core.net`), then check it — what a fiber-sharing test needs, unlike
/// `check_ok`'s bare (unlinked) source.
fn check_linked(src: &str) -> Result<(), TypeError> {
    let tokens = Lexer::tokenize(src).unwrap();
    let program = parse(&tokens).unwrap();
    let (program, _sources) = crate::modules::link(program, std::path::Path::new("."), None)
        .expect("import linking failed");
    TypeChecker::new().check_program(&program).map(|_| ())
}

/// The binding name a fiber-sharing rejection names — panics on any other outcome, so a
/// test asserting the WRONG binding (or no rejection at all) fails loudly rather than
/// silently passing.
fn shared_across_fibers_name(src: &str) -> String {
    match check_linked(src) {
        Err(TypeError::SharedAcrossFibers { name, .. }) => name,
        other => panic!("expected a SharedAcrossFibers rejection, got {other:?}"),
    }
}

#[test]
fn test_fiber_handler_reading_a_plain_global_is_rejected() {
    let src = "<< core.net\n\
               hits := 0\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59401\", connection => < hits == 0 ? $ : $ >)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_fiber_handler_writing_a_plain_global_is_rejected() {
    let src = "<< core.net\n\
               hits := 0\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59402\", connection => < hits := hits + 1 >)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_fiber_handler_touching_a_global_two_calls_deep_is_rejected() {
    let src = "<< core.net\n\
               hits := 0\n\
               bump = () -> $ => < hits := hits + 1 >\n\
               callBump = () -> $ => < bump() >\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59403\", connection => < callBump() >)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_fiber_handler_capturing_a_local_of_the_enclosing_block_is_rejected() {
    let src = "<< core.net\n\
               ^ = () -> Num => <\n  \
                 hits := 0\n  \
                 net.@tcpServe(\"127.0.0.1:59404\", connection => < hits := hits + 1 >)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_fiber_handler_reaching_an_atomic_global_is_accepted() {
    let src = "<< core.net\n\
               @hits := 0\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59405\", connection => < hits := hits + 1 >)\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_fiber_handler_reading_an_immutable_global_record_is_accepted_despite_its_setter() {
    // `=` freezes the value even though `Tally` declares a `:=` setter — the deep-
    // immutability invariant this check reuses.
    let src = "<< core.net\n\
               Tally = { count :: Num, bump := () => < it.count := it.count + 1 > }\n\
               board = Tally { count = 0 }\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59406\", connection => < board.count == 0 ? $ : $ >)\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_fiber_handler_declaring_its_own_local_is_accepted() {
    let src = "<< core.net\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59407\", connection => <\n    \
                   visits := 0\n    \
                   visits := visits + 1\n    \
                   $\n  \
                 >)\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_a_program_with_no_server_is_unaffected_by_the_fiber_sharing_check() {
    let src = "hits := 0\n\
               bump = () -> $ => < hits := hits + 1 >\n\
               ^ = () -> Num => <\n  \
                 bump()\n  \
                 hits\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_fiber_handler_named_by_a_local_variable_is_still_checked() {
    // `h` is a local (non-top-level) named handler, not an inline lambda — no-hoisting
    // means it is declared above the call that passes it, so the check must find it there.
    let src = "<< core.net\n\
               hits := 0\n\
               ^ = () -> Num => <\n  \
                 h = (connection :: net.Connection) => < hits := hits + 1 >\n  \
                 net.@tcpServe(\"127.0.0.1:59408\", h)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_fiber_handler_lookup_ignores_a_same_named_local_in_an_unrelated_closure() {
    // `setup`'s own `h` sits earlier in `^`'s body than `wrapper`'s, but it is not
    // `wrapper`'s `h` — the lookup must resolve the name against the call's OWN
    // enclosing function, `wrapper`, not whichever same-named declaration comes first
    // in a flat search of `^`'s whole body.
    let src = "<< core.net\n\
               count := 0\n\
               ^ = () -> Num => <\n  \
                 setup = () -> Bool => <\n    \
                   h = (c :: net.Connection) => < 0 >\n    \
                   true\n  \
                 >\n  \
                 wrapper = () -> Num => <\n    \
                   h = (c :: net.Connection) => < count := count + 1 >\n    \
                   net.@tcpServe(\"127.0.0.1:59411\", h)\n    \
                   0\n  \
                 >\n  \
                 setup()\n  \
                 wrapper()\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "count");
}

#[test]
fn test_fiber_handler_capturing_an_atomic_local_of_the_enclosing_block_is_accepted() {
    // The mirror of `test_fiber_handler_reaching_an_atomic_global_is_accepted`, one
    // scope down: a BLOCK-local atomic binding is exactly as safe as a top-level one.
    let src = "<< core.net\n\
               ^ = () -> Num => <\n  \
                 @hits := 0\n  \
                 net.@tcpServe(\"127.0.0.1:59409\", connection => < hits := hits + 1 >)\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_fiber_handler_local_of_its_own_is_accepted_despite_a_same_named_local_elsewhere() {
    // `helper`'s own `count` is a totally different binding from the handler's own
    // `count`, declared in an unrelated sibling function of `^` — sharing a name with a
    // local the handler itself declares must not make it look captured.
    let src = "<< core.net\n\
               ^ = () -> Num => <\n  \
                 net.@tcpServe(\"127.0.0.1:59413\", connection => <\n    \
                   count := 0\n    \
                   count := count + 1\n    \
                   $\n  \
                 >)\n  \
                 helper = () -> Num => <\n    \
                   count := 5\n    \
                   count\n  \
                 >\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

#[test]
fn test_http_serve_handler_writing_a_plain_global_is_rejected() {
    // `http.@serve` joins `net.@tcpServe` on the same fiber-launching accept loop, so the
    // check must reject a non-atomic global its handler writes exactly the same way.
    let src = "<< core.http\n\
               hits := 0\n\
               ^ = () -> Num => <\n  \
                 http.@serve(\"127.0.0.1:59414\", request => <\n    \
                   hits := hits + 1\n    \
                   http.Response.reply(http.OK, \"ok\")\n  \
                 >)\n  \
                 0\n\
               >";
    assert_eq!(shared_across_fibers_name(src), "hits");
}

#[test]
fn test_http_serve_handler_writing_an_atomic_global_is_accepted() {
    let src = "<< core.http\n\
               @hits := 0\n\
               ^ = () -> Num => <\n  \
                 http.@serve(\"127.0.0.1:59415\", request => <\n    \
                   hits := hits + 1\n    \
                   http.Response.reply(http.OK, \"ok\")\n  \
                 >)\n  \
                 0\n\
               >";
    assert!(check_linked(src).is_ok());
}

// Dead-code (`checker::dead_functions`) tests: QN352 (never reachable) and QN353
// (reachable only from test blocks), and the shapes that must NOT be reported.

#[test]
fn test_a_never_called_top_level_function_is_rejected() {
    let err =
        check_ok("sift = (n :: Num) -> Num => < n + 1 >\n^ = () -> Num => < 0 >").unwrap_err();
    assert!(matches!(err, TypeError::NeverReachable { ref name, .. } if name == "sift"));
}

#[test]
fn test_a_called_top_level_function_is_not_reported() {
    assert!(
        check_ok("sift = (n :: Num) -> Num => < n + 1 >\n^ = () -> Num => < sift(1) >").is_ok()
    );
}

#[test]
fn test_an_exported_top_level_function_is_never_reported_even_if_uncalled() {
    // `>>` is a module's public surface: an importer elsewhere may call it, so it is
    // never dead, however this program itself treats it.
    assert!(check_ok(">> sift = (n :: Num) -> Num => < n + 1 >\n^ = () -> Num => < 0 >").is_ok());
}

#[test]
fn test_a_module_with_no_entry_point_and_no_export_is_not_checked() {
    // No `^` and no export: `ast::reachability::reachable_functions` returns `None`, the
    // same signal codegen's own pruning reads as "keep everything, a later program may
    // call any of it" — so a private helper nothing here calls is not reported.
    assert!(check_ok("sift = (n :: Num) -> Num => < n + 1 >").is_ok());
}

#[test]
fn test_a_module_with_no_entry_point_whose_export_calls_its_helper_is_accepted() {
    assert!(
        check_ok(
            "sift = (n :: Num) -> Num => < n + 1 >\n\
             >> grind = (n :: Num) -> Num => < sift(n) >"
        )
        .is_ok()
    );
}

#[test]
fn test_a_module_with_no_entry_point_whose_helper_nothing_calls_is_rejected() {
    // No `^`, so the module's own `>>` exports are the only roots — `sift` is reachable
    // from neither `grind` nor anything else, exactly as dead as it would be under a
    // real entry point.
    let err = check_ok(
        ">> grind = (n :: Num) -> Num => < n * 2 >\nsift = (n :: Num) -> Num => < n + 1 >",
    )
    .unwrap_err();
    assert!(matches!(err, TypeError::NeverReachable { ref name, .. } if name == "sift"));
}

#[test]
fn test_a_function_reachable_only_from_a_test_block_is_reported_as_such() {
    // `run`/`build`/`check` erase `test.describe` blocks before compiling — this is what
    // that erasure leaves behind: `describe` looked alive on the page, but nothing
    // `run`/`build`/`check` actually keeps calls it.
    let src = "\
<< core.test
describe = (result :: Result) -> Text => <
  result ? | Ok(text) => text | NotOk(_) => \"none\"
>
test.describe(\"describe\", () => <
  test.it(\"ok\", () => < expect(describe(Ok(\"home\")), equals(\"home\")) >)
>)
^ = () -> Num => < 0 >
";
    let err = check_ok(src).unwrap_err();
    assert!(
        matches!(err, TypeError::ReachableOnlyFromTests { ref name, .. } if name == "describe")
    );
}

#[test]
fn test_a_method_calling_nothing_is_never_reported() {
    // Only a top-level `FunctionDeclaration` is a dead-code candidate — a type's method
    // rides along with its declaration (a root, per `ast::reachability`), never checked
    // for reachability on its own.
    assert!(
        check_ok(
            "Box = { size :: Num, unused = => < 0 > }\n^ = () -> Num => < Box { size = 1 }.size >"
        )
        .is_ok()
    );
}

#[test]
fn test_an_uncalled_top_level_binding_is_never_reported() {
    // A top-level binding's value is always emitted (never pruned by codegen), and this
    // check only ever looks at `FunctionDeclaration` items.
    assert!(check_ok("unused = 5\n^ = () -> Num => < 0 >").is_ok());
}
