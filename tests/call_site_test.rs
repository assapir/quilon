//! The `Site` call-site facility: a trailing `site :: Site` parameter the compiler fills
//! in with the location of the call that left it off.
//!
//! This is the general mechanism `core.test`'s assertions are built on (their reporting is
//! covered by `assert_test.rs`); the cases here are about the facility itself — that the
//! location is the CALLER's, that it propagates through a chain of forwarding wrappers
//! (track-caller, as opposed to reporting the innermost hop), and that a `Site` parameter
//! nothing could fill in is rejected rather than quietly demanding an argument.

mod common;
use common::{TEST_FILE, assert_exit, assert_type_error, type_error_message};
use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;

/// The filled-in site is the CALL's location, not the callee's definition. The exit code
/// is the reported line, so the assertion is the line number of the call itself.
#[test]
fn a_site_parameter_receives_the_calls_own_line() {
    // The `line()` call is on line 3 of this source.
    let src = "\
line = (site :: Site) -> Num => site.line
^ = () -> Num =>
  line()
";
    assert_exit(src, 3);
}

/// Column and width frame the call: the column is where the call starts (1-based) and the
/// width is how many characters it spans — `frame()` at column 3 is 7 characters wide.
#[test]
fn a_site_parameter_frames_the_call_with_column_and_width() {
    let src = "\
frame = (site :: Site) -> Num => site.column * 100 + site.width
^ = () -> Num =>
  frame()
";
    assert_exit(src, 3 * 100 + 7);
}

/// The site names the file the call is in and carries the text of its line, which is what
/// lets a failure report show the call with a caret under it.
#[test]
fn a_site_parameter_carries_the_callers_file_and_source_line() {
    let src = format!(
        "\
here = (site :: Site) -> Bool => site.file == \"{TEST_FILE}\" && site.excerpt.contains(\"here()\")
^ = () -> Num =>
  here() ? 9 : 0
"
    );
    assert_exit(&src, 9);
}

/// Track-caller: a wrapper that FORWARDS its own `site` reports where IT was called, not
/// where it calls the inner function. Two hops of forwarding still name the user's call.
#[test]
fn an_explicitly_forwarded_site_propagates_through_a_chain() {
    // The `outer()` call is on line 5; the forwarding hops are on lines 1-3.
    let src = "\
inner = (site :: Site) -> Num => site.line
middle = (site :: Site) -> Num => inner(site)
outer = (site :: Site) -> Num => middle(site)
^ = () -> Num =>
  outer()
";
    assert_exit(src, 5);
}

/// A hop that does NOT forward its site reports its own call instead — the location is
/// always the call that left the argument off, never a chain the program didn't ask for.
#[test]
fn a_hop_that_does_not_forward_reports_its_own_call() {
    let src = "\
inner = (site :: Site) -> Num => site.line
outer = (site :: Site) -> Num => inner()
^ = () -> Num =>
  outer()
";
    assert_exit(src, 2);
}

/// Overload dispatch sees through the filled-in argument: each member takes a trailing
/// `Site`, and a one-argument call still resolves by that argument's type.
#[test]
fn overload_members_can_each_take_a_site() {
    let src = "\
kind = (n :: Num, site :: Site) -> Num => 1
kind = (t :: Text, site :: Site) -> Num => 2
^ = () -> Num => kind(\"text\") * 10 + kind(7)
";
    assert_exit(src, 21);
}

/// A `Site` may also be passed explicitly from the outside — it is an ordinary record
/// value, so a caller can hand on a location it received.
#[test]
fn a_site_can_be_passed_explicitly() {
    let src = "\
line = (site :: Site) -> Num => site.line
relay = (site :: Site) -> Num => line(site)
^ = () -> Num =>
  relay()
";
    assert_exit(src, 4);
}

/// `Site` is a built-in type: nameable in any signature with no import, and its fields
/// read like any record's.
#[test]
fn site_needs_no_import() {
    let src = "\
sum = (site :: Site) -> Num => site.line + site.column
^ = () -> Num => sum()
";
    assert_exit(src, 2 + 18);
}

/// Only the LAST parameter can be filled in from the call site, so a `Site` before it
/// could never be omitted — rejected at compile time rather than silently requiring an
/// explicit location.
#[test]
fn a_site_parameter_before_the_last_is_rejected() {
    assert_type_error(
        "\
odd = (site :: Site, n :: Num) -> Num => n
^ = () -> Num => odd(1)
",
    );
}

/// A declaration nested in another function is emitted as a local function or lifted into
/// a closure — neither is reached through the named-call path that fills a site in, so a
/// `Site` parameter on one is rejected rather than left unfillable.
#[test]
fn a_site_parameter_on_a_nested_declaration_is_rejected() {
    assert_type_error(
        "\
^ = () -> Num => <
  f = (n :: Num, site :: Site) -> Num => n
  f(1)
>
",
    );
}

/// A lambda is a function VALUE, called through its binding rather than by name, so it
/// cannot receive a call site either.
#[test]
fn a_site_parameter_on_a_lambda_is_rejected() {
    assert_type_error(
        "\
apply = (f :: Num) -> Num => f
^ = () -> Num => <
  g = (n :: Num, site :: Site) => n + site.line
  g(1)
>
",
    );
}

/// A record method never receives a call site either (the receiver is the first argument,
/// and dispatch is by type, not by name) — so a `Site` parameter on a method is rejected.
#[test]
fn a_site_parameter_on_a_method_is_rejected() {
    assert_type_error(
        "\
Box = {
  n :: Num
  where = (site :: Site) -> Num => site.line
}
^ = () -> Num => 0
",
    );
}

/// Adopting the facility must not cost a recursive function its loop: a self-call that
/// leaves off its own trailing `Site` is still a self-call, so it is still lowered to a
/// jump. Without that, this overflows the stack instead of returning — the language has no
/// loop construct, so tail recursion IS iteration.
#[test]
fn a_recursive_function_taking_a_site_still_becomes_a_loop() {
    let src = "\
countdown = (n :: Num, acc :: Num, site :: Site) -> Num =>
  n == 0 ? acc : countdown(n - 1, acc + 1)
^ = () -> Num => countdown(500000, 0) - 499958
";
    assert_exit(src, 42);
}

/// A `Site` parameter is never part of the signature a CALLER has to satisfy, so no
/// diagnostic may ask for one: a wrong-arity call to `core.test`'s `failAt(message, site)`
/// reports its parameters as `(Text)`, not `(Text, Site)`.
#[test]
fn a_diagnostic_never_asks_for_the_filled_in_argument() {
    let error = type_error_message("<< core.test\n^ = () -> $ => test.failAt()\n");
    assert!(
        error.contains("expected 1") && !error.contains("Site"),
        "the arity must be counted without the filled-in Site, got: {error}"
    );
}

/// A `Site` is a compile-time constant, so a call pays nothing for it: the record is a
/// read-only global and the call passes its address. No `__alloc` for the site, no stores —
/// which is what lets a program assert as often as it likes.
#[test]
fn a_call_site_is_a_constant_not_an_allocation() {
    let src = "line = (site :: Site) -> Num => site.line\n^ = () -> Num => line()\n";
    let ir = ir_for(src);
    let entry = ir
        .split("define internal double @\"^\"")
        .nth(1)
        .expect("the entry point must be emitted");
    assert!(
        !entry.contains("@__alloc"),
        "filling in a call site must not allocate, got:\n{entry}"
    );
    assert!(
        site_constant(&ir).is_some(),
        "the site must be a read-only constant global, got:\n{ir}"
    );
}

/// A `Site` is read-only: writing one of its fields is a compile error, however the value
/// was reached. Records are handles that alias, so a write through a `:=` rebinding would
/// otherwise be a write to the constant the call site was lowered to.
#[test]
fn writing_a_site_field_is_rejected() {
    let error = type_error_message(
        "\
tamper = (site :: Site) -> Num => <
  site.line := 99
  site.line
>
^ = () -> Num => tamper()
",
    );
    assert!(
        error.contains("a `Site` is read-only"),
        "a Site field write must be refused as read-only, got: {error}"
    );

    // The aliasing route to the same write — rebinding the parameter `:=` first — is
    // stopped one step earlier, at the rebinding: a parameter's value cannot be made
    // mutable.
    let error = type_error_message(
        "\
tamper = (site :: Site) -> Num => <
  s := site
  s.line := 99
  s.line
>
^ = () -> Num => tamper()
",
    );
    assert!(
        error.contains("parameter 'site'"),
        "rebinding a Site parameter `:=` must be refused as a mutable alias, got: {error}"
    );
}

/// `Site` is a built-in type name, so a program declaring its own is a duplicate
/// definition — the same way `Result` is taken.
#[test]
fn a_program_cannot_declare_its_own_site_type() {
    assert_type_error("Site = { x :: Num }\n^ = () -> Num => 0\n");
}

/// Codegen must survive having NO source map: a program assembled in memory (as the
/// IR-only codegen tests are) still emits a filled-in site, with an empty `file` standing
/// for "unknown" and the position left at 1:1 so a reader's arithmetic holds. (Reading the
/// fields back needs the type oracle, exactly as any record does; what is asserted here is
/// that the missing source map is not itself a failure.)
#[test]
fn a_call_site_without_a_source_map_still_compiles() {
    let src = "line = (site :: Site) -> Num => site.line + site.column\n^ = () -> Num => line()\n";
    let ir = ir_for(src);
    assert!(
        ir.contains("@line(ptr %site)"),
        "the Site-taking function must still be emitted, got:\n{ir}"
    );
    let site = site_constant(&ir).unwrap_or_else(|| panic!("no site constant in:\n{ir}"));
    assert!(
        site.contains("double 1.000000e+00"),
        "an unknown location must still carry a 1-based position, got: {site}"
    );
}

/// The LLVM IR for `src`, generated with NO source map installed — the IR-only path the
/// codegen tests use, where a call site resolves to the documented "unknown" location.
fn ir_for(src: &str) -> String {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parse(&tokens).expect("parsing failed");
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator
        .generate(&program)
        .unwrap_or_else(|e| panic!("codegen without a source map failed: {e}"))
}

/// The IR line defining a call site's constant global, if the module has one. Matched by
/// the global's name (`@site.<file>.<start>.<end>`, as opposed to the `@site.str` byte
/// constants its `Text` fields point at) rather than by the struct layout, so adding a
/// `Site` field does not break every test that only cares that a site IS a constant.
fn site_constant(ir: &str) -> Option<&str> {
    ir.lines().find(|line| {
        line.strip_prefix("@site.")
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            && line.contains("constant")
    })
}
