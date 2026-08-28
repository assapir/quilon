//! The endpoint contract of `lo <- hi`: an endpoint that is not a whole number an `i64`
//! can hold is REFUSED, never truncated. `1.5 <- 3.9` was silently `[1, 2, 3]`, and a NaN
//! or out-of-`i64` end reached `fptosi` as poison — a constant one folded to poison before
//! the allocation was even sized, which segfaulted.
//!
//! A literal end is a compile error; a computed one aborts at the range expression. The
//! aborting path terminates the process, so those tests drive the real `quilon` binary as
//! a subprocess (an in-process JIT run would take the harness down with it).

mod common;
use common::{
    assert_exit, build_and_run_native, build_native, run_program, tool_available,
    type_error_message,
};

/// The two literal shapes that used to segfault or truncate, and the message each earns.
const FRACTIONAL: &str = "^ = () -> Num => <\n  r = 1.5 <- 3.9\n  r.size\n>";
const TOO_WIDE: &str = "^ = () -> Num => <\n  r = 1 <- 10000000000000000000\n  r.size\n>";
/// A NaN end, which is only NaN once the division has run.
const NAN_END: &str = "^ = () -> Num => <\n  r = 1 <- (0.0 / 0.0)\n  r.size\n>";
/// A fractional end the checker cannot see: `7 / 2` is 3.5 at runtime.
const COMPUTED_FRACTIONAL: &str = "^ = () -> Num => <\n  n = 7 / 2\n  r = 1 <- n\n  r.size\n>";

#[test]
fn a_fractional_literal_endpoint_is_a_compile_error() {
    assert_eq!(
        type_error_message(FRACTIONAL),
        "a range endpoint must be a whole number (got 1.5)"
    );
}

#[test]
fn a_literal_endpoint_wider_than_an_i64_is_a_compile_error() {
    assert_eq!(
        type_error_message(TOO_WIDE),
        "a range endpoint must be a whole number that fits 64 bits (got 10000000000000000000)"
    );
}

/// Either end is checked, and a negated literal is still a literal.
#[test]
fn both_ends_are_checked_including_a_negated_literal() {
    assert_eq!(
        type_error_message("^ = () -> Num => <\n  r = 0.5 <- 4\n  r.size\n>"),
        "a range endpoint must be a whole number (got 0.5)"
    );
    assert_eq!(
        type_error_message("^ = () -> Num => <\n  r = 0 <- -2.5\n  r.size\n>"),
        "a range endpoint must be a whole number (got -2.5)"
    );
}

#[test]
fn a_nan_endpoint_aborts_with_a_located_message() {
    let (code, stderr, _) = run_program("range_nan", NAN_END);
    assert_eq!(code, 1, "a NaN endpoint must exit 1: {stderr}");
    assert!(
        stderr.contains(":2:7:\na range endpoint must be a whole number (got NaN)"),
        "the report must locate the range and name the value, got: {stderr}"
    );
    assert!(
        stderr.contains("2 |   r = 1 <- (0.0 / 0.0)") && stderr.contains("      ^^^^^^^^^^^^^^^"),
        "the report must show the range with a caret under it, got: {stderr}"
    );
}

#[test]
fn a_computed_fractional_endpoint_aborts_with_a_located_message() {
    let (code, stderr, _) = run_program("range_computed", COMPUTED_FRACTIONAL);
    assert_eq!(
        code, 1,
        "a computed fractional endpoint must exit 1: {stderr}"
    );
    assert!(
        stderr.contains(":3:7:\na range endpoint must be a whole number (got 3.5)"),
        "the report must locate the range and name the value, got: {stderr}"
    );
}

/// A computed end past `i64` earns the width message rather than the whole-number one:
/// 10^19 is whole, it just has nothing to count in.
#[test]
fn a_computed_endpoint_wider_than_an_i64_aborts() {
    let (code, stderr, _) = run_program(
        "range_wide",
        "^ = () -> Num => <\n  n = 1000000000000000000 * 10\n  r = 1 <- n\n  r.size\n>",
    );
    assert_eq!(
        code, 1,
        "a too-wide computed endpoint must exit 1: {stderr}"
    );
    assert!(
        stderr.contains("a range endpoint must be a whole number that fits 64 bits"),
        "the report must say the endpoint does not fit, got: {stderr}"
    );
}

/// The check lives in the runtime both back ends share, and the compile errors happen
/// before either — so a native build refuses all three shapes exactly as `run` does.
#[test]
fn a_native_build_refuses_the_same_endpoints() {
    if !tool_available("clang") {
        eprintln!("skipping the native endpoint checks: clang is not on PATH");
        return;
    }
    let (code, stdout) = build_and_run_native("range_nan_native", NAN_END);
    assert_eq!(
        code, 1,
        "a native build must exit 1 on a NaN endpoint: {stdout}"
    );

    for (tag, source) in [
        ("range_fractional_native", FRACTIONAL),
        ("range_wide_native", TOO_WIDE),
    ] {
        let (build, _) = build_native(tag, source);
        assert_eq!(build.code, 1, "`quilon build` must refuse {tag}");
        assert!(
            build
                .stderr
                .contains("a range endpoint must be a whole number"),
            "the build must say why, got: {}",
            build.stderr
        );
    }
}

/// Whole endpoints are untouched by any of this — including negative ones, a descending
/// range, and ends only known at runtime.
#[test]
fn whole_endpoints_are_unaffected() {
    // [-2, -1, 0, 1, 2]: size 5, first -2, last 2 → 5 + 2 - 2 + 2 = 7.
    assert_exit(
        "^ = () -> Num => <\n  r = -2 <- 2\n  r.size + r[0] + r[4] + 2\n>",
        7,
    );
    // A computed WHOLE end passes the same check: 6 / 2 is exactly 3.
    assert_exit(
        "^ = () -> Num => <\n  n = 6 / 2\n  r = 1 <- n\n  r.size\n>",
        3,
    );
    // Descending still descends, and `i64::MIN`/`i64::MAX`-scale ends are not the point:
    // a single-point range at a legal extreme still builds.
    assert_exit("^ = () -> Num => <\n  r = 4 <- 1\n  r[0]\n>", 4);
}
