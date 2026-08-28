//! Checked allocation, end to end: an array whose backing store cannot be sized or cannot
//! be had must be a CLEAR runtime error — a message on stderr and exit status 1 — never a
//! one-byte block the fill then writes past, and never a null `data` pointer paired with a
//! non-zero length. The failing path terminates the process, so these tests spawn the real
//! `quilon` binary (an in-process JIT run would take the harness down with it).

mod common;
use common::{build_and_run_native, run_program, tool_available};

/// A range of 2e18 elements of 8 bytes is 1.6e19 bytes — more than an `i64` holds.
const OVERFLOWING_RANGE: &str = "^ = () -> Num => <\n  xs = 1 <- 2000000000000000000\n  xs.size\n>";

#[test]
fn an_element_count_times_its_size_that_overflows_aborts() {
    let (code, stderr, _) = run_program("alloc_overflow", OVERFLOWING_RANGE);
    assert_eq!(code, 1, "an unrepresentable size must exit 1: {stderr}");
    assert!(
        stderr.contains("allocation too large: 2000000000000000000 elements of 8 bytes"),
        "stderr must name the count and the element size, got: {stderr}"
    );
}

/// The check lives in the runtime both paths share, so a native build refuses the same
/// size — it exits, where before it died on a signal.
#[test]
fn a_native_build_refuses_the_same_size() {
    if !tool_available("clang") {
        eprintln!("skipping the native allocation check: clang is not on PATH");
        return;
    }
    let (code, _) = build_and_run_native("alloc_overflow_native", OVERFLOWING_RANGE);
    assert_eq!(code, 1, "a native build must exit 1 on the same size");
}

/// 1e18 elements of 8 bytes IS representable — and unobtainable. The collector returns
/// null, which used to become an array whose `data` was null and whose `size` said 1e18.
#[test]
fn an_allocation_the_collector_cannot_satisfy_aborts() {
    let (code, stderr, _) = run_program(
        "alloc_oom",
        "^ = () -> Num => <\n  xs = 1 <- 1000000000000000000\n  xs.size\n>",
    );
    assert_eq!(code, 1, "a failed allocation must exit 1: {stderr}");
    assert!(
        stderr.contains("out of memory: cannot allocate 8000000000000000000 bytes"),
        "stderr must name the size that could not be had, got: {stderr}"
    );
}

/// The check is on the size, not on the program: ordinary arrays — a literal, a range, a
/// method result — still allocate and read back exactly as before.
#[test]
fn ordinary_array_allocation_is_unaffected() {
    let (code, stderr, _) = run_program(
        "alloc_ok",
        "^ = () -> Num => <\n  xs = [1, 2, 3]\n  big = 1 <- 1000\n  doubled = xs.map(n => n * 2)\n  xs.size + big.size / 100 + doubled[2]\n>",
    );
    assert_eq!(code, 19, "3 + 10 + 6 = 19, got {code}: {stderr}");
    assert!(stderr.is_empty(), "no error expected, got: {stderr}");
}
