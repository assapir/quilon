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
fn a_user_files_own_overload_of_the_same_bare_name_is_unaffected() {
    // `__exit` is not a reserved name — a user file may give it its OWN overload member
    // at a different signature; only a call matching the COMPILER's own signature is the
    // intrinsic itself.
    let src = "__exit = (message :: Text) -> Text => < message >\n\
               ^ = () -> Num => < __exit(\"spreadsheet gremlin\").length >";
    let tokens = quilon::lexer::Lexer::tokenize(src).expect("lexing failed");
    let program = quilon::parser::parse(&tokens).expect("parsing failed");
    quilon::typechecker::TypeChecker::new()
        .check_program(&program)
        .expect("a user's own differently-typed overload of a bare `__` name must compile");
}
