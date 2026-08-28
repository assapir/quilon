//! Checked allocation, end to end: an array whose backing store cannot be had must be a
//! CLEAR runtime error — a message on stderr and exit status 1 — never a one-byte block the
//! fill then writes past, and never a null `data` pointer paired with a non-zero length. The
//! failing path terminates the process, so these tests spawn the real `quilon` binary (an
//! in-process JIT run would take the harness down with it).
//!
//! A range is the only `.qn` construct that names an element count without writing the
//! elements, and its endpoints are capped at 2^53 (the largest whole number a `Num` holds
//! exactly), so the widest array a program can ask for is ~1.4e17 bytes — unobtainable, but
//! representable. `__alloc_array`'s `count * elem_size` overflow guard therefore sits above
//! what a `.qn` program reaches through a range; it still guards the runtime's own array
//! builders (`alloc_slots` — `Text.split`, `argv`, the Map/Set snapshots), whose counts come
//! from text length, the OS, and live data.

mod common;
use common::{build_and_run_native, run_program, tool_available};

/// The largest array a program can ask for: 2^53 elements of 8 bytes, 7.2e16 bytes. The
/// collector returns null, which used to become an array whose `data` was null while its
/// `size` said otherwise.
const UNOBTAINABLE_RANGE: &str = "^ = () -> Num => <\n  xs = 1 <- 9007199254740992\n  xs.size\n>";

#[test]
fn an_allocation_the_collector_cannot_satisfy_aborts() {
    let (code, stderr, _) = run_program("alloc_oom", UNOBTAINABLE_RANGE);
    assert_eq!(code, 1, "a failed allocation must exit 1: {stderr}");
    assert!(
        stderr.contains("out of memory: cannot allocate 72057594037927936 bytes"),
        "stderr must name the size that could not be had, got: {stderr}"
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
    let (code, _) = build_and_run_native("alloc_oom_native", UNOBTAINABLE_RANGE);
    assert_eq!(code, 1, "a native build must exit 1 on the same size");
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
