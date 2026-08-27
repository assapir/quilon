//! `print`/`eprint`/`write` take anything renderable: one printing rule, the type's
//! `` ` `` render member, shared with string interpolation.

mod common;

use common::{
    assert_type_error, build_and_run_native, run_program_named, tool_available, type_error_message,
};

/// A user type with a `` ` `` render member prints through it — no corelib involvement,
/// and the same rendering reaches interpolation, `eprint` and `write`.
#[test]
fn a_render_member_is_what_makes_a_type_printable() {
    let run = run_program_named(
        "render_member.qn",
        r#"
<< core.io

Money = {
  amount :: Num,
  currency :: Text,
  ` = () -> Text => "`it.amount` `it.currency`"
}

^ = () -> Num => <
  price = Money { amount = 12, currency = "EUR" }
  print(price)
  print("costs `price`")
  price |> write(stdout)
  eprint(price)
  0
>
"#,
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "12 EUR\ncosts 12 EUR\n12 EUR");
    assert_eq!(run.stderr, "12 EUR\n");
}

/// A type WITHOUT a render member keeps the built-in default for its shape: a record shows
/// its type name, a sum its variant, an array its elements.
#[test]
fn a_type_without_a_render_member_uses_the_default_for_its_shape() {
    let run = run_program_named(
        "render_default.qn",
        r#"
<< core.io

Point = {
  x :: Num,
  y :: Num
}

Shape = Circle(Num) / Square(Num)

^ = () -> Num => <
  print(Point { x = 1, y = 2 })
  print(Circle(3))
  print([1, 2, 3])
  print($)
  0
>
"#,
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "Point\nCircle\n[1, 2, 3]\n$\n");
}

/// A function value is the one thing that does not render, and the error says so.
#[test]
fn a_function_value_has_no_rendering() {
    for source in [
        "<< core.io\ndouble = (n :: Num) -> Num => n * 2\n^ = () -> Num => <\n  print(double)\n  0\n>",
        "<< core.io\ndouble = (n :: Num) -> Num => n * 2\n^ = () -> Num => <\n  double |> write(stdout)\n  0\n>",
    ] {
        assert_type_error(source);
    }
}

/// The arity a CALLER sees is what the built-in claims: a trailing `Site` the compiler fills
/// in does not smuggle a member in behind it. (Such a member type-checked while the call it
/// would answer went to the built-in — a definition reachable from nowhere.)
#[test]
fn a_trailing_site_does_not_hide_a_member_behind_the_builtin() {
    let message = type_error_message(
        r#"
<< core.io
print = (label :: Text, at :: Site) -> Num => 7
^ = () -> Num => 0
"#,
    );
    assert!(
        message.contains("render member"),
        "expected the render-member guidance, got: {message}"
    );
}

/// A call above a user set of the same name is still reported as one resolving too early,
/// not as a wrong argument count against the built-in.
#[test]
fn a_call_above_a_user_set_reports_the_definition_order() {
    let message = type_error_message(
        r#"
<< core.io
^ = () -> Num => print(40, 2)
print = (a :: Num, b :: Num) -> Num => a + b
"#,
    );
    assert!(
        message.contains("before its definition"),
        "expected the definition-order error, got: {message}"
    );
}

/// The built-in claims its own arity, so a call at another one — with no user set of that
/// name to answer it — is reported against the built-in rather than as an unknown name.
#[test]
fn a_call_at_another_arity_reports_the_builtins_arity() {
    for source in [
        "<< core.io\n^ = () -> Num => <\n  print()\n  0\n>",
        "<< core.io\n^ = () -> Num => <\n  write(\"raw\")\n  0\n>",
    ] {
        let message = type_error_message(source);
        assert!(
            message.contains("Wrong number of arguments"),
            "expected an arity error, got: {message}"
        );
    }
}

/// `write` renders too, so it is not limited to `Text` — it just adds no newline and
/// passes the rendered bytes through as they are.
#[test]
fn write_renders_a_non_text_value() {
    let run = run_program_named(
        "write_renders.qn",
        r#"
<< core.io

^ = () -> Num => <
  written = write(42, stdout)
  write(true, stdout)
  written
>
"#,
    );
    assert_eq!(run.code, 2, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "42True");
}

/// The rendering a native executable produces is the one the JIT produces.
#[test]
fn rendering_holds_in_a_native_executable() {
    if !tool_available("clang") {
        eprintln!("skipping the native build: clang is not on PATH");
        return;
    }
    let (code, stdout) = build_and_run_native(
        "render_native",
        r#"
<< core.io

Tag = {
  label :: Text,
  ` = () -> Text => "[`it.label`]"
}

^ = () -> Num => <
  print(Tag { label = "ok" })
  write(7, stdout)
>
"#,
    );
    assert_eq!(stdout, "[ok]\n7");
    assert_eq!(code, 1, "one byte written");
}
