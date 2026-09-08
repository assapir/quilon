//! A bare `__`-prefixed compiler intrinsic (`__exit`, `__color_enabled`, and the rest of
//! the internal `BuiltinOverload` names) is corelib-only surface: `core.test`'s harness
//! calls it directly, but a user file naming the same call has no such name in scope.

use quilon::diagnostic::codes::Code;

mod common;
use common::assert_type_error_code;

#[test]
fn a_user_file_calling_exit_directly_is_undefined() {
    assert_type_error_code(
        "^ = () -> Num => <\n  __exit(3)\n  0\n>",
        Code::UndefinedVariable,
    );
}

#[test]
fn a_user_file_calling_color_enabled_directly_is_undefined() {
    assert_type_error_code(
        "^ = () -> Num => < __color_enabled(1) ? 1 : 0 >",
        Code::UndefinedVariable,
    );
}

#[test]
fn a_mismatched_call_is_still_undefined_not_a_no_matching_overload() {
    // A wrong-typed or wrong-arity call must not fall through to overload resolution,
    // which would report `NoMatchingOverload` and name the intrinsic's real signature.
    assert_type_error_code(
        "^ = () -> Num => < __exit(\"x\")  0 >",
        Code::UndefinedVariable,
    );
    assert_type_error_code(
        "^ = () -> Num => < __exit(1, 2)  0 >",
        Code::UndefinedVariable,
    );
}

#[test]
fn a_user_files_own_overload_of_the_same_bare_name_is_unaffected() {
    // `__exit` is not a reserved name — a user file may give it its OWN overload member at
    // a different signature, which then dispatches normally beside the (still unreachable)
    // intrinsic.
    let src = "__exit = (message :: Text) -> Text => < message >\n\
               ^ = () -> Num => < __exit(\"spreadsheet gremlin\").length >";
    let tokens = quilon::lexer::Lexer::tokenize(src).expect("lexing failed");
    let program = quilon::parser::parse(&tokens).expect("parsing failed");
    quilon::typechecker::TypeChecker::new()
        .check_program(&program)
        .expect("a user's own differently-typed overload of a bare `__` name must compile");
}

#[test]
fn a_call_matching_the_builtins_own_signature_is_still_undefined_beside_a_user_overload() {
    // A user file's own `__exit(Text)` does not open the door to the compiler's own
    // `__exit(Num)` member: a call shaped like the INTRINSIC's signature must still be
    // undefined, not dispatch to it.
    assert_type_error_code(
        "__exit = (x :: Text) -> Num => < 4 >\n\
         ^ = () -> Num => < __exit(3) >",
        Code::UndefinedVariable,
    );
}
