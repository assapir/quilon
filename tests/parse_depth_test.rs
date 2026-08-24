//! Recursion-depth guard: deeply nested input must fail with a clean parse error
//! instead of overflowing the native stack and aborting (SIGABRT / core dump).
//!
//! These drive the real `quilon` binary as a subprocess, so a regression that
//! recurses until the stack overflows shows up unmistakably: the process is
//! *killed by a signal* (`status.code()` is `None`) rather than exiting cleanly
//! with a diagnostic. Every "past the limit" case here nests thousands of levels
//! deep — enough that the pre-guard parser aborted with exit 134 on this input.

use std::io::Write;
use std::process::{Command, Output};

/// Write `source` to a temp `.qn` file and run `quilon check` on it. The file
/// lives under the system temp dir, namespaced by pid + `name` so parallel test
/// runs don't collide.
fn check(name: &str, source: &str) -> Output {
    let mut path = std::env::temp_dir();
    path.push(format!("quilon_depth_{}_{}.qn", std::process::id(), name));
    let mut f = std::fs::File::create(&path).expect("create temp .qn");
    f.write_all(source.as_bytes()).expect("write temp .qn");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run quilon");

    let _ = std::fs::remove_file(&path);
    out
}

/// Assert the run failed *cleanly*: it exited with a normal non-zero status (not
/// killed by a signal, which is how a stack overflow manifests), and its stderr
/// carries the depth-guard diagnostic with a `:line:col:` position line and an `error:`
/// message line under it.
fn assert_clean_depth_error(out: &Output, what: &str) {
    // A stack overflow aborts via SIGABRT; on Unix that leaves `code()` == None.
    // A real diagnostic exits with a code (1). This is the crash-vs-diagnostic line.
    assert_eq!(
        out.status.code(),
        Some(1),
        "{what}: process did not exit cleanly (killed by signal => stack overflow regression); \
         status = {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nesting too deep"),
        "{what}: missing depth-guard message; stderr was:\n{stderr}"
    );
    assert!(
        stderr.contains("\nerror: "),
        "{what}: missing the `error:` message line; stderr was:\n{stderr}"
    );
}

#[test]
fn deeply_nested_parens_error_instead_of_crashing() {
    // The exact shape from the bug report: ~2000 nested parens in the entry body.
    let n = 2000;
    let src = format!("^ = () -> Num => {}1{}\n", "(".repeat(n), ")".repeat(n));
    let out = check("parens", &src);
    assert_clean_depth_error(&out, "nested parens");
}

#[test]
fn deeply_nested_arrays_error_instead_of_crashing() {
    // A different construct: nested array literals `[[[ ... ]]]`.
    let n = 2000;
    let src = format!(
        "^ = () -> Num => ({}1{}).size\n",
        "[".repeat(n),
        "]".repeat(n)
    );
    let out = check("arrays", &src);
    assert_clean_depth_error(&out, "nested arrays");
}

#[test]
fn deeply_nested_records_error_instead_of_crashing() {
    // A third construct: nested record literals `{a={a= ... }}`.
    let n = 2000;
    let src = format!(
        "f = () -> Num => {}1{}\n^ = () -> Num => 0\n",
        "{a=".repeat(n),
        "}".repeat(n)
    );
    let out = check("records", &src);
    assert_clean_depth_error(&out, "nested records");
}

#[test]
fn deeply_nested_patterns_error_instead_of_crashing() {
    // Pattern syntax recurses independently of the expression grammar: a
    // constructor pattern's arguments re-enter `parse_pattern` (`Ok(Ok(…))`).
    // This recursion is lighter than the expression chain, so it needs a deeper
    // nesting to overflow — hence the larger count here.
    let n = 50_000;
    let src = format!(
        "^ = () -> Num => 0 ? | {}x{} => 0\n",
        "Ok(".repeat(n),
        ")".repeat(n)
    );
    let out = check("patterns", &src);
    assert_clean_depth_error(&out, "nested patterns");
}

#[test]
fn deeply_chained_prefix_operators_error_instead_of_crashing() {
    // Chained prefix operators (`---…x`) re-enter `parse_unary` directly, outside
    // the `parse_expression` funnel — another independent, lighter recursion.
    let n = 100_000;
    let src = format!("^ = () -> Num => {}1\n", "-".repeat(n));
    let out = check("unary", &src);
    assert_clean_depth_error(&out, "chained prefix operators");
}

#[test]
fn deeply_chained_field_assignments_error_instead_of_crashing() {
    // A `:=` chain (`a.x := b.y := …`) re-enters `parse_assignment` directly,
    // bypassing the `parse_expression` funnel.
    let n = 100_000;
    let src = format!("^ = () -> Num => < a := 0\n{}0\n0 >\n", "a.b := ".repeat(n));
    let out = check("assign", &src);
    assert_clean_depth_error(&out, "chained field assignments");
}

#[test]
fn deeply_nested_block_functions_error_instead_of_crashing() {
    // Nested named functions whose bodies are blocks
    // (`f = () => < g = () => < … > >`) recurse through
    // `parse_block`/`parse_item`/`parse_function_declaration`, not `parse_expression`.
    let n = 20_000;
    let src = format!(
        "^ = () -> Num => {}1{}\n",
        "< f = () -> Num => ".repeat(n),
        " >".repeat(n)
    );
    let out = check("blockfns", &src);
    assert_clean_depth_error(&out, "nested block functions");
}

#[test]
fn moderately_nested_input_still_parses() {
    // Well under the limit: a run of nested parens that flattens to plain `1`.
    // This must sail through parse + type-check and exit successfully, proving the
    // guard only rejects pathological depth, not ordinary nesting.
    let n = 120;
    let src = format!("^ = () -> Num => {}1{}\n", "(".repeat(n), ")".repeat(n));
    let out = check("shallow", &src);
    assert!(
        out.status.success(),
        "moderate nesting should type-check; status = {:?}, stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}
