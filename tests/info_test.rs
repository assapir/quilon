//! `core.info` — what a program can ask about itself.
//!
//! Two things to hold. The ANSWERS have to be real: the target's architecture, OS, pointer
//! width and byte order, spelled the way people say them. And each closed set has to stay a
//! SUM, so a match over one is exhaustive and a wrong variant fails to compile — that is what
//! separates these from a `Text` that happens to read the same.

use quilon::driver::front_end;
use std::io::Write;

mod common;
use common::assert_exit_linked;

/// The front end must REJECT `src` — written to a temp file so its `<<` imports resolve the
/// way they do for a real program.
fn assert_rejected(src: &str) {
    let dir =
        std::env::temp_dir().join(format!("quilon_info_{}_{}", std::process::id(), src.len()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("prog.qn");
    let mut handle = std::fs::File::create(&file).expect("write temp source");
    handle.write_all(src.as_bytes()).expect("write temp source");
    drop(handle);
    let result = front_end(&file);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err(), "expected a compile error for:\n{src}");
}

#[test]
fn every_member_answers() {
    // 1 + 2 + 4 + 8 = 15. A missing answer drops its bit.
    assert_exit_linked(
        "<< core.info\n\
         ^ = () -> Num => (info.platform().name().size > 0 ? 1 : 0) + (info.os().name().size > 0 ? 2 : 0) \
           + (info.quilonVersion().size > 0 ? 4 : 0) + (info.endianness().name().size > 0 ? 8 : 0)",
        15,
    );
}

#[test]
fn a_closed_set_is_matchable() {
    // The point of the sums. Every variant named, no catch-all.
    assert_exit_linked(
        "<< core.info\n\
         ^ = () -> Num => info.endianness() ? | info.Little => 7 | info.Big => 1",
        7,
    );
}

#[test]
fn a_match_missing_a_variant_is_a_compile_error() {
    // Exhaustiveness is what a `Text` could not have given us.
    assert_rejected("<< core.info\n^ = () -> Num => info.endianness() ? | info.Little => 7");
}

#[test]
fn a_variant_that_does_not_exist_is_a_compile_error() {
    // A triple spells Apple's OS `darwin`; there is deliberately no such variant, and naming
    // one is caught at compile time rather than reading back an unexpected string.
    assert_rejected("<< core.info\n^ = () -> Num => info.os() ? | info.Darwin(_) => 1 | _ => 0");
}

#[test]
fn an_unknown_says_what_it_saw() {
    // The payload is the point: a target with no variant of its own still reports which one
    // it was, rather than collapsing to the word "unknown".
    assert_exit_linked(
        "<< core.info\n\
         ^ = () -> Num => info.WtfPlatform(\"sparc64\").name() == \"sparc64\" ? 7 : 1",
        7,
    );
}

#[test]
fn pointer_width_is_sixty_four_or_thirty_two() {
    assert_exit_linked(
        "<< core.info\n^ = () -> Num => info.pointerWidth() ? | info.Width64 => 64 | info.Width32 => 32",
        64,
    );
}

#[test]
fn pointer_width_bits_is_a_num() {
    assert_exit_linked(
        "<< core.info\n^ = () -> Num => info.pointerWidth().bits() / 8",
        8,
    );
}

#[test]
fn os_is_never_spelled_the_way_a_triple_spells_it() {
    assert_exit_linked(
        "<< core.info\n^ = () -> Num => info.os().name() == \"darwin\" ? 1 : 0",
        0,
    );
}

#[test]
fn two_reads_agree() {
    // Being constants, they cannot drift between reads — unlike `now()`.
    assert_exit_linked(
        "<< core.info\n\
         ^ = () -> Num => (info.platform().name() == info.platform().name() ? 1 : 0) \
           + (info.os().name() == info.os().name() ? 2 : 0) \
           + (info.quilonVersion() == info.quilonVersion() ? 4 : 0)",
        7,
    );
}

#[test]
fn run_mode_is_jit_under_the_jit() {
    // `assert_exit_linked` runs through the in-process JIT, which is the half of `runMode`
    // reachable from a test; `examples/info.qn` covers the other half when built.
    assert_exit_linked(
        "<< core.info\n^ = () -> Num => info.runMode() ? | info.Jit => 7 | info.Aot => 1",
        7,
    );
}

#[test]
fn the_version_is_the_compilers_own() {
    let source = format!(
        "<< core.info\n^ = () -> Num => info.quilonVersion() == \"{}\" ? 7 : 1",
        env!("CARGO_PKG_VERSION")
    );
    assert_exit_linked(&source, 7);
}

#[test]
fn the_import_is_required() {
    // Unlike `core.time`, this module declares real types and functions, so the names come
    // from the import rather than from the compiler.
    assert_rejected("^ = () -> Num => info.pointerWidth().bits()");
}
