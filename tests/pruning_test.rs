//! A function nothing can reach from `^` is not emitted — and everything that *is*
//! reachable still is, however indirectly it is reached.
//!
//! The failure mode this guards against is silent: a function dropped because the analysis
//! did not see the thing that reaches it — an operator overload, a render override called
//! only by interpolation, a helper called only from a method — shows up as a link error, or
//! under the JIT as a missing symbol, in a program that compiled perfectly well before.

use quilon::lexer::Lexer;
use quilon::parser;

mod common;
use common::{assert_exit, assert_exit_linked};

/// The LLVM IR for `src`, which is where a pruned function is visibly absent.
fn emit(src: &str) -> String {
    let context = inkwell::context::Context::create();
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let types = quilon::typechecker::TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let mut codegen = quilon::codegen::CodeGenerator::new(&context, "test");
    codegen.set_type_table(types);
    codegen.generate(&program).expect("codegen failed")
}

fn defines(ir: &str, name: &str) -> bool {
    ir.lines().any(|line| {
        line.starts_with("define")
            && (line.contains(&format!("@\"{name}\"(")) || line.contains(&format!("@{name}(")))
    })
}

#[test]
fn an_unreachable_function_is_not_emitted() {
    let ir = emit(
        "used = (n :: Num) -> Num => n + 1\nunused = (n :: Num) -> Num => n + 2\n^ = () -> Num => used(1)",
    );
    assert!(defines(&ir, "used"), "the called function must be emitted");
    assert!(
        !defines(&ir, "unused"),
        "nothing reaches `unused`, so it should not be emitted:\n{ir}"
    );
}

#[test]
fn reachability_follows_a_chain_of_calls() {
    let ir = emit(concat!(
        "third = (n :: Num) -> Num => n + 3\n",
        "second = (n :: Num) -> Num => third(n) + 2\n",
        "first = (n :: Num) -> Num => second(n) + 1\n",
        "orphan = (n :: Num) -> Num => third(n)\n",
        "^ = () -> Num => first(0)"
    ));
    for live in ["first", "second", "third"] {
        assert!(
            defines(&ir, live),
            "`{live}` is reachable through the chain"
        );
    }
    assert!(!defines(&ir, "orphan"), "`orphan` is called by nothing");
}

// There is no test for a top-level function passed as a value, because the language has no
// way to express one: an array method requires a lambda argument, and a parameter cannot be
// annotated with a function type. A function value therefore only ever reaches other
// functions through a lambda's body, which the global-function-value test below covers.

#[test]
fn an_operator_overload_reached_only_by_the_operator_survives() {
    // `p + q` is the sole mention of the `+` member: no call by name anywhere. The
    // operator is a member of `Money`, and a type's members ride along with it.
    assert_exit(
        concat!(
            "Money = { amount :: Num, + = (other :: Money) -> Money => Money { amount = it.amount + other.amount } }\n",
            "^ = () -> Num => <\n",
            "  total = Money { amount = 40 } + Money { amount = 2 }\n",
            "  total.amount\n",
            ">"
        ),
        42,
    );
}

#[test]
fn a_helper_called_only_from_a_method_survives() {
    // Methods ride along with their type declaration, so a method body is a root.
    assert_exit(
        concat!(
            "scale = (n :: Num) -> Num => n * 10\n",
            "Box = { size :: Num, scaled = => scale(it.size) }\n",
            "^ = () -> Num => Box { size = 4 }.scaled()"
        ),
        40,
    );
}

#[test]
fn a_helper_called_only_from_a_global_function_value_survives() {
    assert_exit(
        concat!(
            "bump = (n :: Num) -> Num => n + 1\n",
            "step = (n :: Num) => bump(n)\n",
            "^ = () -> Num => step(6)"
        ),
        7,
    );
}

#[test]
fn a_render_override_reached_only_by_interpolation_survives() {
    // The `` ` `` method is never called by name — interpolation lowers to it.
    assert_exit_linked(
        concat!(
            "<< core.test\n",
            "Tag = { id :: Num, ` = => \"tag\" }\n",
            "^ = () -> $ => <\n",
            "  t = Tag { id = 1 }\n",
            "  assert(\"a `t` b\", equals(\"a tag b\"))\n",
            ">"
        ),
        0,
    );
}

#[test]
fn an_overload_member_reached_only_by_dispatch_survives() {
    // Both members are mentioned by the one name `describe`; each call resolves to a
    // different one by argument type.
    assert_exit(
        concat!(
            "describe = (n :: Num) -> Num => 1\n",
            "describe = (t :: Text) -> Num => 2\n",
            "^ = () -> Num => describe(1) + describe(\"x\")"
        ),
        3,
    );
}

#[test]
fn a_methods_receiver_is_not_a_mention_of_a_top_level_name() {
    // Reading the receiver as a top-level mention keeps the harness's `it` function, and the
    // harness's whole chain behind it, in any program that declares a type with a method.
    let ir = emit(concat!(
        "it = (name :: Text) -> Num => name.size\n",
        "Box = { size :: Num, doubled = => it.size * 2 }\n",
        "^ = () -> Num => Box { size = 21 }.doubled()"
    ));
    assert!(
        !defines(&ir, "it"),
        "the receiver in `it.size` must not keep the top-level `it`:\n{ir}"
    );
}

#[test]
fn a_call_of_a_top_level_function_named_it_still_reaches_it() {
    // The other side of the narrowing: callee position is where a bare `it` CAN name a
    // top-level function, and dropping it there would break the harness itself.
    assert_exit(
        concat!(
            "it = (name :: Text) -> Num => name.size\n",
            "Box = { size :: Num, doubled = => it.size * 2 }\n",
            "^ = () -> Num => Box { size = 20 }.doubled() + it(\"ab\")"
        ),
        42,
    );
}

#[test]
fn a_pipeline_into_a_top_level_function_named_it_reaches_it() {
    // `x |> f` desugars to a call of `f`, so a bare name on the right of a pipe is a callee
    // too — the second position where a bare `it` names a top-level function.
    assert_exit(
        concat!(
            "it = (n :: Num) -> Num => n + 1\n",
            "Box = { size :: Num, doubled = => it.size * 2 }\n",
            "^ = () -> Num => Box { size = 20 }.doubled() + 1 |> it"
        ),
        42,
    );
}

#[test]
fn a_module_with_no_entry_point_keeps_everything() {
    // Nothing is reachable from an entry point that does not exist, so a module compiled on
    // its own must not be emptied out — a later program may call any of it.
    let ir = emit("first = (n :: Num) -> Num => n + 1\nsecond = (n :: Num) -> Num => n + 2");
    assert!(defines(&ir, "first"));
    assert!(defines(&ir, "second"));
}

#[test]
fn imported_corelib_still_provides_what_the_program_uses() {
    // The point of the change: importing a module no longer emits all of it, so what the
    // program does use has to keep working. (The AOT side of this — every example through
    // the real linker, all of which now prune — is `examples_test`'s JIT/AOT comparison.)
    assert_exit_linked(
        "<< core.io\n<< core.test\n^ = () -> $ => <\n  print(\"hi\")\n  assert(1 + 1, equals(2))\n>",
        0,
    );
    assert_exit_linked(
        "<< core.io\n<< core.test\n^ = () -> $ => <\n  print(\"hi\")\n  assert(\"a\", equals(\"a\"))\n>",
        0,
    );
}
