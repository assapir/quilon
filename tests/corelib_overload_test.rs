//! A corelib built-in's name a user also defines: module overload sets are CLOSED, so a
//! program's own `print` / `write` / `now` is an ordinary, unrelated function — the
//! built-in stays reachable only as `io.print` / `io.write` / `time.now`. The internal
//! `__` primitives belong to no module and remain overload members on the old terms.

mod common;

use common::{assert_exit_linked, build_and_run_native, tool_available, type_error_message};

#[test]
fn a_users_output_name_is_an_ordinary_function() {
    // The output built-ins are `core.io`'s exports now; a user's own `print` (even at
    // the built-in's arity) is just a function, dispatched like any other.
    assert_exit_linked(
        r#"
print = (x :: Num) -> Num => x + 1
^ = () -> Num => print(41)
"#,
        42,
    );
}

#[test]
fn a_user_wrapper_reaches_the_builtin_through_the_module() {
    // Building on a module is composition: the wrapper holds the import and delegates.
    assert_exit_linked(
        r#"
<< core.io

write = (content :: Text) -> Num => io.write(content, io.stdout)

^ = () -> Num => <
  write("raw")
>
"#,
        3,
    );
}

#[test]
fn the_builtin_now_reads_the_clock_beside_an_unrelated_user_now() {
    assert_exit_linked(
        r#"
<< core.time

now = (scale :: Num) -> Num => time.now() * scale

^ = () -> Num => <
  ~ The clock is monotonic and never negative, so both functions ran.
  time.now() >= 0 && now(0) == 0 ? 7 : 0
>
"#,
        7,
    );
}

#[test]
fn a_user_now_at_the_builtins_signature_is_legal() {
    // Closed sets mean no collision: a nullary user `now` is not a duplicate of
    // `core.time.now` — it is a different name entirely, and the call picks the user's.
    assert_exit_linked(
        r#"
<< core.time
now = () -> Num => 7
^ = () -> Num => time.now() >= 0 ? now() : 0
"#,
        7,
    );
}

#[test]
fn an_under_annotated_overload_of_an_internal_primitive_is_reported() {
    // The `__` primitives are still compiler-provided overload sets, so a user member
    // that cannot be dispatched — no parameter annotations — is rejected as before.
    let message = type_error_message(
        r#"
__color_enabled = (x) => 5
^ = () -> Num => 0
"#,
    );
    assert!(
        message.contains("must annotate every parameter"),
        "expected the annotation requirement, got: {message}"
    );
}

#[test]
fn the_internal_primitives_are_still_overload_members() {
    // `__exit`/`__color_enabled` are internal (no module exports them) and keep the old
    // rule: a user definition at another signature joins the set and is dispatched to.
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
    // A user's `write` is an ordinary function, and a self-call in it is still lowered
    // to a loop. A million frames would overflow.
    assert_exit_linked(
        r#"
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

write = (label :: Text) -> Num => io.write(label + "!", io.stdout)

^ = () -> Num => <
  ~ Both forms write: the built-in takes a file descriptor, the user's own `write`
  ~ picks one itself and hands the built-in the suffixed Text.
  written = io.write("built-in ", io.stdout)
  written + write("user")
>
"#,
    );
    assert_eq!(stdout, "built-in user!", "unexpected program output");
    assert_eq!(
        code, 14,
        "9 bytes from the built-in, plus the user function's 5"
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
  written = "hi" |> io.write(io.stdout)
  time.now() >= 0 ? written : 0
>
"#,
        2,
    );
}
