//! End-to-end proof of the block-scope join: a `< >` block that directly launches a
//! value-returning `@` primitive joins every launch it made — `allSettled`, never
//! cancelled — before its value flows out, and reports every fault among them, in launch
//! order, each naming its own launch site.
//!
//! Follows `tests/read_stdin_test.rs`'s own subprocess harness: a program that fails loudly
//! calls `__exit`, which would kill the test runner if it ran in-process, and these tests
//! need a controlled stdin besides.

mod common;

use common::{build_and_run_native_with_stderr, temp_ql, tool_available};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Two `@readStdin()` launches, the first bound but never read. Stdin is a single serial
/// stream (the stdin gate serializes readers), so the SECOND read only sees "world" if the
/// FIRST one actually ran to completion and consumed "hello" first — proving the unused
/// launch is not merely dropped.
const FIRST_UNUSED_SECOND_ASSERTED: &str = r#"
<< core.io
<< core.test

^ = () -> Num => <
  first = @readStdin()
  second = @readStdin()
  assert(second, equals("world"))
  0
>
"#;

/// Two `@readStdin()` launches, both bound but never read (never individually forced) — so
/// the only thing that can join them is the block's own close. Run against a stdin that can
/// never be read successfully (see `run_with_unreadable_stdin`), both fault.
const TWO_UNFORCED_READS: &str = r#"
<< core.io

^ = () -> Num => <
  first = @readStdin()
  second = @readStdin()
  0
>
"#;

/// One `@readStdin()`, forced directly by an `assert` rather than left for the block's own
/// close — proving a strict force that discovers a fault reports and exits too.
const ONE_READ_FORCED_DIRECTLY: &str = r#"
<< core.io
<< core.test

^ = () -> Num => <
  line = @readStdin()
  assert(line, equals("hello"))
  0
>
"#;

/// A self-tail-recursive function whose body directly launches: `readCount`'s own body
/// block needs a launch scope opened and joined once per CALL, and the loop lowering below
/// gives it exactly that — a fresh scope entered at the loop header, joined right before
/// every back-edge (see `generate_block`/`emit_tail_self_call`) — so this still runs as a
/// genuine constant-stack loop, not a stack-growing recursive call. With no input piped,
/// every `@readStdin()` yields "" (end-of-input) so the total stays 0 regardless of depth.
const SELF_TAIL_RECURSIVE_WITH_A_DIRECT_LAUNCH: &str = r#"
<< core.io
<< core.test

readCount = (remaining :: Num, total :: Num) -> Num => <
  line = @readStdin()
  newTotal = total + line.size
  remaining <= 1 ? newTotal : readCount(remaining - 1, newTotal)
>

^ = () -> Num => <
  assert(readCount(3, 0), equals(0))
  0
>
"#;

/// The same shape, but deep enough (20,000 calls) that a stack-growing recursive call would
/// overflow even the SEED fiber's 8 MiB — let alone a spawned launch fiber's 512 KiB — while
/// a genuine constant-stack loop runs it in the same bounded space regardless of depth.
const DEEP_SELF_TAIL_RECURSIVE_WITH_A_DIRECT_LAUNCH: &str = r#"
<< core.io
<< core.test

readCount = (remaining :: Num, total :: Num) -> Num => <
  line = @readStdin()
  newTotal = total + line.size
  remaining <= 1 ? newTotal : readCount(remaining - 1, newTotal)
>

^ = () -> Num => <
  assert(readCount(20000, 0), equals(0))
  0
>
"#;

/// An unread launch, then heavy allocation before the block's close settles it. Under a
/// native (`-O3`) build, `hearsay`'s own alloca — the only thing that would otherwise keep
/// its deferred cell reachable — has no further use once dead-store elimination sees it is
/// never read again, so between the read settling and the block's join the cell must stay
/// reachable some other way, or a collection triggered by the array churn below frees it out
/// from under the join.
const UNREAD_LAUNCH_THEN_HEAVY_ALLOCATION: &str = r#"
<< core.io

^ = () -> Num => <
  hearsay = @readStdin()
  (1 <- 100000).each(i => <
    junk = [i, i, i, i, i, i, i, i]
    0
  >)
  0
>
"#;

/// `quilon run <file>`, piping `input` to stdin, capturing the exit code and both streams.
fn run_with_piped_stdin(file: &Path, input: &[u8]) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn quilon run");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write to child stdin");
    let output = child.wait_with_output().expect("wait for quilon run");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `quilon run <file>`, with stdin redirected from a directory — a source `@readStdin` can
/// never successfully read: `read(2)` on a directory fails immediately (`EISDIR` on Linux),
/// never blocking, so this deterministically drives the read-failure path without racing a
/// timeout or a signal.
fn run_with_unreadable_stdin(file: &Path) -> (Option<i32>, String, String) {
    let directory = File::open(std::env::temp_dir()).expect("open a directory for reading");
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(Stdio::from(directory))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn quilon run");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_bound_but_unused_launch_settles_before_the_block_returns() {
    let file = temp_ql("unused_launch", FIRST_UNUSED_SECOND_ASSERTED);
    let (code, _, stderr) = run_with_piped_stdin(&file, b"hello\nworld\n");
    assert_eq!(
        code,
        Some(0),
        "the unused first read must consume \"hello\" before the second reads \"world\": {stderr}"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_sibling_launch_still_settles_after_an_earlier_one_faults() {
    let file = temp_ql("sibling_settles", TWO_UNFORCED_READS);
    let (code, _, stderr) = run_with_unreadable_stdin(&file);
    assert_eq!(code, Some(5), "a read fault exits 5: {stderr}");
    assert_eq!(
        stderr.matches("QN505").count(),
        2,
        "both launches must settle (and fault) at the block's close — not just the first: \
         {stderr}"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn faults_from_two_launches_report_in_launch_order() {
    let file = temp_ql("fault_order", TWO_UNFORCED_READS);
    let (code, _, stderr) = run_with_unreadable_stdin(&file);
    assert_eq!(code, Some(5), "a read fault exits 5: {stderr}");
    // `first = @readStdin()` is line 5, `second = @readStdin()` is line 6 of the source
    // above — the first launch's own report must appear before the second's.
    let first_at = stderr
        .find(":5:")
        .unwrap_or_else(|| panic!("expected the first launch's own site in: {stderr}"));
    let second_at = stderr
        .find(":6:")
        .unwrap_or_else(|| panic!("expected the second launch's own site in: {stderr}"));
    assert!(
        first_at < second_at,
        "faults must report in launch order (first launched, first reported): {stderr}"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_directly_forced_fault_reports_and_exits() {
    let file = temp_ql("forced_fault", ONE_READ_FORCED_DIRECTLY);
    let (code, _, stderr) = run_with_unreadable_stdin(&file);
    assert_eq!(code, Some(5), "a read fault exits 5: {stderr}");
    assert!(
        stderr.contains("QN505"),
        "expected a read-failure report: {stderr}"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn self_tail_recursive_function_with_a_direct_launch_still_runs_correctly() {
    let file = temp_ql("tco_launch", SELF_TAIL_RECURSIVE_WITH_A_DIRECT_LAUNCH);
    let (code, _, stderr) = run_with_piped_stdin(&file, b"");
    assert_eq!(code, Some(0), "the loop must still pass: {stderr}");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn an_unread_launchs_cell_survives_heavy_allocation_until_the_join_reads_it() {
    if !tool_available("clang") && !tool_available("gcc") {
        eprintln!("skipping native GC-safety gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    }
    let (code, _, stderr) =
        build_and_run_native_with_stderr("unread_gc", UNREAD_LAUNCH_THEN_HEAVY_ALLOCATION);
    assert_eq!(
        code, 0,
        "the unread launch's cell must survive to the join, not be collected: {stderr}"
    );
}

#[test]
fn self_tail_recursive_function_with_a_direct_launch_runs_in_constant_stack() {
    let file = temp_ql(
        "deep_tco_launch",
        DEEP_SELF_TAIL_RECURSIVE_WITH_A_DIRECT_LAUNCH,
    );
    let (code, _, stderr) = run_with_piped_stdin(&file, b"");
    assert_eq!(
        code,
        Some(0),
        "20,000 calls must still pass in bounded stack, not overflow: {stderr}"
    );
    let _ = std::fs::remove_file(&file);
}
