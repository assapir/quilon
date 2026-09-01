//! The endpoint contract of `lo <- hi`: an end that is not a whole number a `Num` holds
//! exactly — fractional, NaN, infinite, or past 2^53 — is REFUSED, never truncated and never
//! a size derived from poison.
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

/// The limit the message names, spelled once.
const LIMIT: &str = "a range endpoint must be a whole number a Num holds exactly, at most \
                     9007199254740992 in magnitude";

/// Every endpoint the CHECKER can settle from a literal, and the message each earns.
#[test]
fn a_literal_endpoint_a_num_cannot_hold_is_a_compile_error() {
    for (source, expected) in [
        (
            "^ = () -> Num => <\n  r = 1.5 <- 3.9\n  r.size\n>",
            "a range endpoint must be a whole number (got 1.5)".to_string(),
        ),
        (
            "^ = () -> Num => <\n  r = 0 <- -2.5\n  r.size\n>",
            "a range endpoint must be a whole number (got -2.5)".to_string(),
        ),
        (
            // Between 2^53 and 2^63: a whole number, and one an `i64` would have held — the
            // case the old 64-bit rule wrongly accepted.
            "^ = () -> Num => <\n  r = 1 <- 100000000000000000\n  r.size\n>",
            format!("{LIMIT} (got 100000000000000000)"),
        ),
        (
            "^ = () -> Num => <\n  r = 1 <- 10000000000000000000\n  r.size\n>",
            format!("{LIMIT} (got 10000000000000000000)"),
        ),
        (
            "^ = () -> Num => <\n  r = -10000000000000000000 <- 0\n  r.size\n>",
            format!("{LIMIT} (got -10000000000000000000)"),
        ),
    ] {
        assert_eq!(type_error_message(source), expected, "for:\n{source}");
    }
}

/// 2^53 itself is exactly representable, so it is a LEGAL endpoint — the bound is inclusive.
/// A one-element range at the limit builds and reads back.
#[test]
fn the_limit_itself_is_a_legal_endpoint() {
    assert_exit(
        "^ = () -> Num => <\n  r = 9007199254740992 <- 9007199254740992\n  r.size\n>",
        1,
    );
}

/// Every endpoint only the RUNTIME can settle, and the message each earns.
#[test]
fn a_computed_endpoint_a_num_cannot_hold_aborts() {
    for (tag, source, expected) in [
        (
            "range_nan",
            NAN_END,
            "a range endpoint must be a whole number (got NaN)".to_string(),
        ),
        (
            "range_fractional",
            "^ = () -> Num => <\n  n = 7 / 2\n  r = 1 <- n\n  r.size\n>",
            "a range endpoint must be a whole number (got 3.5)".to_string(),
        ),
        (
            // Between 2^53 and 2^63, computed rather than written.
            "range_past_exact",
            "^ = () -> Num => <\n  n = 100000000000000 * 1000\n  r = 1 <- n\n  r.size\n>",
            format!("{LIMIT} (got 100000000000000000)"),
        ),
        (
            "range_wide",
            "^ = () -> Num => <\n  n = 1000000000000000000 * 10\n  r = 1 <- n\n  r.size\n>",
            format!("{LIMIT} (got 10000000000000000000)"),
        ),
        (
            // An infinity is whole; what it lacks is a magnitude a `Num` holds exactly.
            "range_infinite",
            "^ = () -> Num => <\n  n = 1.0 / 0.0\n  r = 1 <- n\n  r.size\n>",
            format!("{LIMIT} (got inf)"),
        ),
    ] {
        let (code, stderr, _) = run_program(tag, source);
        assert_eq!(code, 1, "{tag} must exit 1: {stderr}");
        assert!(
            stderr.contains(&expected),
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
/// computed end exactly as `run` does.
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

/// The LAZY method lowerings validate endpoints through the same shared header the
/// materializing path uses, so a bad computed end aborts with the SAME message and the
/// same range-expression frame — whether the range feeds `.each`, `.reduce`, or `.map`.
#[test]
fn a_lazily_consumed_range_validates_its_endpoints_the_same_way() {
    for (tag, source) in [
        (
            "lazy_nan_each",
            "^ = () -> Num => <\n  (1 <- (0.0 / 0.0)).each(n => n)\n  0\n>",
        ),
        (
            "lazy_nan_reduce",
            "^ = () -> Num => (1 <- (0.0 / 0.0)).reduce(0, (acc, n) => acc + n)",
        ),
        (
            "lazy_nan_map",
            "^ = () -> Num => <\n  r = (1 <- (0.0 / 0.0)).map(n => n)\n  r.size\n>",
        ),
    ] {
        let (code, stderr, _) = run_program(tag, source);
        assert_eq!(code, 1, "{tag} must exit 1: {stderr}");
        assert!(
            stderr.contains("a range endpoint must be a whole number (got NaN)"),
            "{tag} must say why, got: {stderr}"
        );
    }
}

/// A LITERAL bad end on a lazily-consumed range is still settled by the checker, at
/// compile time — the lazy lowering changes nothing ahead of codegen.
#[test]
fn a_literal_bad_endpoint_on_a_lazily_consumed_range_is_a_compile_error() {
    assert_eq!(
        type_error_message("^ = () -> Num => <\n  r = (1.5 <- 3.9).map(n => n)\n  r.size\n>"),
        "a range endpoint must be a whole number (got 1.5)"
    );
}
