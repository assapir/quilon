//! The extent contract of `lo <- hi`: an end that is not a whole number an `i64` holds, and
//! a pair of ends spanning more elements than a count of them, are REFUSED — never
//! truncated, and never a size derived from poison.
//!
//! What the checker can evaluate is a compile error; anything computed aborts at the range
//! expression. The aborting path terminates the process, so those tests drive the real
//! `quilon` binary as a subprocess (an in-process JIT run would take the harness down).

mod common;
use common::{
    assert_exit, build_and_run_native, position, run_program, tool_available, type_error_message,
};

/// A NaN end, which is only NaN once the division has run — the one shape here that
/// reaches the emitted check rather than the checker.
const NAN_END: &str = "^ = () -> Num => <\n  r = 1 <- (0.0 / 0.0)\n  r.size\n>";

/// Every extent the CHECKER can settle from literal ends, and the message each earns.
#[test]
fn a_literal_extent_that_cannot_be_counted_is_a_compile_error() {
    for (source, expected) in [
        (
            "^ = () -> Num => <\n  r = 1.5 <- 3.9\n  r.size\n>",
            "a range endpoint must be a whole number (got 1.5)",
        ),
        (
            "^ = () -> Num => <\n  r = 0 <- -2.5\n  r.size\n>",
            "a range endpoint must be a whole number (got -2.5)",
        ),
        (
            "^ = () -> Num => <\n  r = 1 <- 10000000000000000000\n  r.size\n>",
            "a range endpoint must be a whole number that fits 64 bits (got 10000000000000000000)",
        ),
        (
            // Both ends are legal on their own; there are just more elements between them
            // than a count holds.
            "^ = () -> Num => <\n  r = -5000000000000000000 <- 5000000000000000000\n  r.size\n>",
            "a range from -5000000000000000000 to 5000000000000000000 has more elements than a \
             64-bit count holds",
        ),
    ] {
        assert_eq!(type_error_message(source), expected, "for:\n{source}");
    }
}

/// Every extent only the RUNTIME can settle, and the message each earns.
#[test]
fn a_computed_extent_that_cannot_be_counted_aborts() {
    for (tag, source, expected) in [
        (
            "range_nan",
            NAN_END,
            "a range endpoint must be a whole number (got NaN)",
        ),
        (
            "range_fractional",
            "^ = () -> Num => <\n  n = 7 / 2\n  r = 1 <- n\n  r.size\n>",
            "a range endpoint must be a whole number (got 3.5)",
        ),
        (
            "range_wide",
            "^ = () -> Num => <\n  n = 1000000000000000000 * 10\n  r = 1 <- n\n  r.size\n>",
            "a range endpoint must be a whole number that fits 64 bits (got 10000000000000000000)",
        ),
        (
            // An infinity is whole; what it lacks is an `i64` to count in.
            "range_infinite",
            "^ = () -> Num => <\n  n = 1.0 / 0.0\n  r = 1 <- n\n  r.size\n>",
            "a range endpoint must be a whole number that fits 64 bits (got inf)",
        ),
        (
            "range_span",
            "^ = () -> Num => <\n  lo = 0 - 5000000000000000000\n  r = lo <- 5000000000000000000\n  r.size\n>",
            "a range from -5000000000000000000 to 5000000000000000000 has more elements than a \
             64-bit count holds",
        ),
    ] {
        let (code, stderr, _) = run_program(tag, source);
        assert_eq!(code, 1, "{tag} must exit 1: {stderr}");
        assert!(
            stderr.contains(expected),
            "{tag} must say why, got: {stderr}"
        );
    }
}

/// The report says WHERE: the range expression's own line and column, then the source line
/// with a caret run under it — the frame a bad `array[i]` prints.
#[test]
fn an_abort_reports_the_range_expression() {
    let (code, stderr, path) = run_program("range_located", NAN_END);
    assert_eq!(code, 1);
    let expected = format!(
        "{}\na range endpoint must be a whole number (got NaN)",
        position(&path, 2, 7)
    );
    assert!(
        stderr.contains(&expected),
        "the report must locate the range, got: {stderr}"
    );
    assert!(
        stderr.contains("2 |   r = 1 <- (0.0 / 0.0)") && stderr.contains(&"^".repeat(15)),
        "the report must show the range with a caret under it, got: {stderr}"
    );
}

/// The emitted check is in the runtime both back ends share, so a native build refuses the
/// computed end exactly as `run` does. (The literal shapes never reach a back end — the
/// front end `build` and `run` share rejects them, which the compile-error test covers.)
#[test]
fn a_native_build_refuses_a_computed_endpoint() {
    if !tool_available("clang") {
        eprintln!("skipping the native endpoint check: clang is not on PATH");
        return;
    }
    let (code, stdout) = build_and_run_native("range_nan_native", NAN_END);
    assert_eq!(code, 1, "a native build must exit 1 on a NaN end: {stdout}");
}

/// Ends that ARE whole are untouched by any of this — negative, and computed.
/// (`tests/ranges_test.rs` covers ordinary range behaviour; these are the shapes the new
/// check could plausibly have broken.)
#[test]
fn whole_endpoints_are_unaffected() {
    // [-2, -1, 0, 1, 2]: size 5 + first -2 + last 2 + 2 = 7.
    assert_exit(
        "^ = () -> Num => <\n  r = -2 <- 2\n  r.size + r[0] + r[4] + 2\n>",
        7,
    );
    // A computed end passes the same check when what it computes is whole: 6 / 2 is 3.
    assert_exit(
        "^ = () -> Num => <\n  n = 6 / 2\n  r = 1 <- n\n  r.size\n>",
        3,
    );
}
