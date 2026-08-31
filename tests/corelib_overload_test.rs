//! A corelib built-in a user also defines: `now` is an overload MEMBER, not a reserved
//! name, and the output family (`print`/`eprint`/`write`) claims only its own arity.
//!
//! The failure this exists for: codegen used to intercept `write` and `now` by name,
//! before any dispatch, so a program's own function of that name was replaced by the
//! intrinsic — loudly for `write` (a Num argument reaching the byte writer), silently for
//! `now` (a clock reading returned in place of the program's value), both after the type
//! checker had already resolved the call to the user's definition.

mod common;

use common::{assert_exit_linked, build_and_run_native, tool_available, type_error_message};

#[test]
fn defining_an_output_builtin_at_its_own_arity_is_rejected() {
    // The built-in already accepts every renderable value there, so a member would have no
    // argument types left to claim; the diagnostic points at the render member instead.
    for source in [
        r#"
<< core.io
print = (x :: Text) -> $ => $
^ = () -> Num => 0
"#,
        r#"
<< core.io
eprint = (n :: Num) -> $ => $
^ = () -> Num => 0
"#,
        r#"
<< core.io
write = (content :: Text, fd :: Num) -> Num => 0
^ = () -> Num => 0
"#,
    ] {
        let message = type_error_message(source);
        assert!(
            message.contains("render member"),
            "expected the render-member guidance, got: {message}"
        );
    }
}

#[test]
fn a_user_member_leaves_the_builtin_reachable_from_its_own_body() {
    // The one-argument member picks the file descriptor itself and calls the built-in
    // two-argument member, which still lowers to the runtime writer.
    assert_exit_linked(
        r#"
<< core.io

write = (content :: Text) -> Num => write(content, stdout)

^ = () -> Num => <
  write("raw")
>
"#,
        3,
    );
}

#[test]
fn the_builtin_now_still_reads_the_clock_beside_a_user_member() {
    assert_exit_linked(
        r#"
<< core.time

now = (scale :: Num) -> Num => now() * scale

^ = () -> Num => <
  ~ The clock is monotonic and never negative, so both members ran.
  now() >= 0 && now(0) == 0 ? 7 : 0
>
"#,
        7,
    );
}

#[test]
fn a_definition_of_the_builtin_signature_is_a_duplicate() {
    // The built-in occupies its own signature in the set — redefining it is the ordinary
    // duplicate-definition error, not a silent win for either side.
    let message = type_error_message(
        r#"
<< core.time
now = () -> Num => 7
^ = () -> Num => 0
"#,
    );
    assert!(
        message.contains("Duplicate definition"),
        "expected a duplicate-definition error, got: {message}"
    );
}

#[test]
fn an_under_annotated_definition_of_a_builtin_name_is_reported() {
    // A definition the compiler cannot make a member of — no parameter annotations — is a
    // user's mistake, not the corelib's inert placeholder, and says so. (The corelib's own
    // declarations are recognized by where they come from, so they are never confused with
    // one of these.) `now` collides with the built-in's own empty signature;
    // `__color_enabled`'s unannotated parameter is what stops it becoming a member.
    for (source, expected) in [
        (
            r#"
<< core.time
now = () => 42
^ = () -> Num => now()
"#,
            "Duplicate definition",
        ),
        (
            r#"
__color_enabled = (x) => 5
^ = () -> Num => 0
"#,
            "must annotate every parameter",
        ),
    ] {
        let message = type_error_message(source);
        assert!(
            message.contains(expected),
            "expected {expected:?}, got: {message}"
        );
    }
}

#[test]
fn the_internal_primitives_follow_the_same_rule() {
    // `__exit`/`__color_enabled` are internal (no module exports them), but they are
    // members on the same terms: a user definition at another signature is dispatched to,
    // rather than replaced by the intrinsic — which used to reach codegen as a panic.
    assert_exit_linked(
        r#"
__color_enabled = (label :: Text) -> Num => 7

^ = () -> Num => <
  __color_enabled("x")
>
"#,
        7,
    );
}

#[test]
fn a_user_write_recurses_as_a_loop() {
    // The tail-call analysis asks the same question call lowering does, so a self-call in
    // a user's `write` is still lowered to a loop. A million frames would overflow.
    assert_exit_linked(
        r#"
<< core.io

write = (n :: Num) -> Num => n == 0 ? 1000000 : write(n - 1)

^ = () -> Num => <
  write(1000000) == 1000000 ? 9 : 0
>
"#,
        9,
    );
}

#[test]
fn dispatch_holds_in_a_native_executable() {
    if !tool_available("clang") {
        eprintln!("skipping the native build: clang is not on PATH");
        return;
    }
    let (code, stdout) = build_and_run_native(
        "write_overload",
        r#"
<< core.io

write = (label :: Text) -> Num => write(label + "!", stdout)

^ = () -> Num => <
  ~ Both forms write: the built-in takes a file descriptor, the user member picks one
  ~ itself and hands the built-in the suffixed Text.
  written = write("built-in ", stdout)
  written + write("user")
>
"#,
    );
    assert_eq!(stdout, "built-in user!", "unexpected program output");
    assert_eq!(
        code, 14,
        "9 bytes from the built-in, plus the user member's 5"
    );
}

/// The corelib placeholder declarations the compiler replaces are not emitted, and the
/// names they document still work without a user definition anywhere.
#[test]
fn the_builtins_work_with_no_user_definition() {
    assert_exit_linked(
        r#"
<< core.io
<< core.time

^ = () -> Num => <
  written = write("hi", stdout)
  now() >= 0 ? written : 0
>
"#,
        2,
    );
}
