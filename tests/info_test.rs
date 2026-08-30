//! `core.info` — the compile-time build facts (`platform`, `os`, `quilonVersion`).
//!
//! Two things to hold. The VALUES have to be real: a program that reads them gets the
//! architecture and OS it runs on, spelled the way people say it. And they have to stay
//! CONSTANTS: each resolves while the program is compiled, so the emitted IR contains the
//! string and no call at all. A regression in the second is invisible from behaviour alone —
//! a runtime lookup would return the same answer — so it is checked against the IR.

use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;

mod common;
use common::assert_exit;

fn gen_ir(source: &str) -> String {
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let program = parse(&tokens).expect("parsing failed");
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator
        .generate(&program)
        .unwrap_or_else(|e| panic!("codegen failed: {e:?}"))
}

#[test]
fn every_member_yields_a_non_empty_text() {
    // 1 + 2 + 4 + 8 = 15. An empty answer from any of them would drop its bit.
    assert_exit(
        "<< core.info\n\
         ^ = () -> Num => (platform().size > 0 ? 1 : 0) + (os().size > 0 ? 2 : 0) \
           + (quilonVersion().size > 0 ? 4 : 0) + (endianness().size > 0 ? 8 : 0)",
        15,
    );
}

#[test]
fn the_import_is_documentation_only() {
    // The members are compiler-provided, like `print` and `now`, so they resolve with no
    // import at all. `<< core.info` declares intent; it does not merge the names.
    assert_exit("^ = () -> Num => platform().size > 0 ? 7 : 1", 7);
}

#[test]
fn a_member_is_a_constant_not_a_call() {
    // The whole point: resolved at compile time. The IR must carry the string and no call.
    let ir = gen_ir("<< core.info\n^ = () -> Text => platform()");
    assert!(
        !ir.contains("@platform"),
        "platform() must not survive as a call:\n{ir}"
    );
    assert!(
        ir.contains("private unnamed_addr constant"),
        "platform() must lower to a string constant:\n{ir}"
    );
}

#[test]
fn two_reads_in_one_program_agree() {
    // Being constants, they cannot drift between reads — unlike `now()`, the other
    // compiler-provided nullary member.
    assert_exit(
        "<< core.info\n\
         ^ = () -> Num => (platform() == platform() ? 1 : 0) + (os() == os() ? 2 : 0) \
           + (quilonVersion() == quilonVersion() ? 4 : 0)",
        7,
    );
}

#[test]
fn os_is_spelled_the_way_people_say_it() {
    // A target triple spells Apple's OS `darwin`; `os()` must translate rather than leak the
    // triple through. Checked on every platform, since the failure is the same everywhere:
    // the raw triple substring reaching the answer.
    assert_exit(
        "<< core.info\n\
         ^ = () -> Num => os() == \"darwin\" ? 1 : 0",
        0,
    );
}

#[test]
fn bits_is_the_targets_pointer_width() {
    // 64 or 32 — every target Quilon builds for is one of the two, and the value comes from
    // LLVM's data layout rather than the architecture's name.
    assert_exit(
        "<< core.info\n^ = () -> Num => bits() == 64 || bits() == 32 ? 7 : 1",
        7,
    );
}

#[test]
fn endianness_is_little_or_big() {
    assert_exit(
        "<< core.info\n\
         ^ = () -> Num => endianness() == \"little\" || endianness() == \"big\" ? 7 : 1",
        7,
    );
}

#[test]
fn bits_is_a_num_not_a_text() {
    // The one member that is not `Text`, so it has to arithmetic like any other `Num`.
    assert_exit("<< core.info\n^ = () -> Num => bits() / 8", 8);
}

#[test]
fn the_version_is_the_compilers_own() {
    // Not a placeholder and not the corelib stub's empty string: the value baked in is the
    // compiler's `CARGO_PKG_VERSION`.
    let source = format!(
        "<< core.info\n^ = () -> Num => quilonVersion() == \"{}\" ? 7 : 1",
        env!("CARGO_PKG_VERSION")
    );
    assert_exit(&source, 7);
}
