//! End-to-end diagnostics gate: drives the real `quilon` binary on deliberately
//! broken programs and asserts the `error[Q…]:` header, the `╭─[path:line:col]` position,
//! the offending source line and its underline reach stderr, and that the process still
//! exits non-zero.

use std::io::Write;
use std::process::Command;

mod common;
use common::position;

/// Write `source` to a temp `.qn` file and run `quilon check` on it. The file
/// lives under the cargo target tmp dir so parallel test runs don't collide.
fn check_output(name: &str, source: &str) -> std::process::Output {
    let mut path = std::env::temp_dir();
    path.push(format!("quilon_diag_{}_{}.qn", std::process::id(), name));
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

/// Return the exit status and diagnostic stream for an invalid program.
fn check(name: &str, source: &str) -> (bool, String) {
    let out = check_output(name, source);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Status goes to stderr, never stdout; without a terminal (a captured pipe, exactly what
/// `Command::output` gives this test) there is NO per-stage line at all — stage progress is
/// a live-terminal-only spinner that leaves no trace — just the closing line naming the
/// file and the elapsed time.
#[test]
fn check_writes_status_to_stderr_not_stdout() {
    let out = check_output("status", "^ = () -> Num => < 0 >\n");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "`quilon check` failed: {stderr}");
    assert!(
        stdout.is_empty(),
        "`quilon check` wrote status to stdout: {stdout}"
    );
    for stage in ["lexing", "parsing", "resolving", "checking"] {
        assert!(
            !stderr.lines().any(|line| line.starts_with(stage)),
            "a per-stage line leaked off a terminal: {stderr}"
        );
    }
    assert_eq!(
        stderr.lines().count(),
        1,
        "off a terminal, stderr is the closing line alone: {stderr}"
    );
    let last = stderr.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("✓ ") && last.contains("quilon_diag_") && last.contains("ms)"),
        "the closing line names the file and the elapsed time: {stderr}"
    );
    assert!(
        !stderr.contains("\x1b["),
        "no color off a terminal: {stderr}"
    );
}

/// `--quiet` silences every status line; a diagnostic still prints.
#[test]
fn quiet_prints_no_status_but_still_the_diagnostic() {
    let run = |source: &str| {
        let mut path = std::env::temp_dir();
        path.push(format!("quilon_diag_quiet_{}.qn", std::process::id()));
        std::fs::write(&path, source).expect("write temp .qn");
        let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
            .args(["--quiet", "check"])
            .arg(&path)
            .output()
            .expect("run quilon");
        let _ = std::fs::remove_file(&path);
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (ok, stderr) = run("^ = () -> Num => < 0 >\n");
    assert!(ok);
    assert!(stderr.is_empty(), "quiet says nothing: {stderr}");

    let (ok, stderr) = run("^ = () -> Num => < # >\n");
    assert!(!ok);
    assert!(stderr.starts_with("error[QN002]:"), "{stderr}");
    assert!(!stderr.contains("lexing"), "{stderr}");
}

#[test]
fn type_error_reports_line_col_and_caret() {
    // `a + true` has no `+` overload for (Num, Bool) — a clear no-match error on line 2.
    let src = "~ comment\nadd = (a :: Num) -> Num => < a + true >\n^ = () -> Num => < add(1) >\n";
    let (ok, stderr) = check("type", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains("error[QN311]: no overload of `+` takes (Num, Bool)"),
        "no coded header: {stderr}"
    );
    assert!(stderr.contains(":2:30]"), "no line:col position: {stderr}");
    // The offending source line is echoed...
    assert!(
        stderr.contains("add = (a :: Num) -> Num => < a + true >"),
        "source line missing: {stderr}"
    );
    // ...with each operand labelled with its type, and the members offered as help.
    assert!(
        stderr.contains("─ Num") && stderr.contains("─ Bool"),
        "operands are labelled: {stderr}"
    );
    assert!(
        stderr.contains("help: the members of `+` are (Num, Num), (Text, Text)"),
        "no help: {stderr}"
    );
}

/// `Num + Text` is the one mix with an idiomatic fix, so the help shows it.
#[test]
fn num_plus_text_suggests_interpolation() {
    let (ok, stderr) = check("interpolate", "^ = () -> Num => < x = 1 + \"x\"  0 >\n");
    assert!(!ok);
    assert!(
        stderr.contains("help: to join a number and text, interpolate"),
        "{stderr}"
    );
}

#[test]
fn lexer_error_reports_line_col_and_caret() {
    // `#` is not a valid token. (`@` used to be invalid, but now marks a deferring
    // primitive like `@sleep`, so it is a real token.)
    let src = "^ = () -> Num => < # >\n";
    let (ok, stderr) = check("lex", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains("error[QN002]: invalid token `#`\n"),
        "{stderr}"
    );
    assert!(stderr.contains(":1:20]"), "no line:col position: {stderr}");
    assert!(stderr.contains('─'), "no underline: {stderr}");
}

/// A parse error names the tokens in the language's own words — a block close is
/// "a block close `>`", never the compiler's internal name for it.
#[test]
fn parse_error_reports_line_col() {
    let src = "^ = () -> Num => < (1 + 2 >\n";
    let (ok, stderr) = check("parse", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains("error[QN100]: expected `)`, found a block close `>`\n"),
        "{stderr}"
    );
    assert!(stderr.contains(":1:27]"), "{stderr}");
    assert!(
        !stderr.contains("BlockClose"),
        "internal names leak: {stderr}"
    );
}

/// A type error inside an IMPORTED module is reported against that module — its path, its
/// line, its source line — not against the file being compiled. Type checking runs over the
/// linked program, so the span belongs to whichever module it came from; rendering every
/// span against the root file named the wrong file and underlined whatever happened to sit
/// at that byte offset.
#[test]
fn a_type_error_in_an_imported_module_names_that_module() {
    let dir = std::env::temp_dir().join(format!("quilon_diag_import_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let module = dir.join("broken_lib.qn");
    std::fs::write(
        &module,
        "~ a module with a type error\n>> broken = (n :: Num) -> Text => < n >\n",
    )
    .expect("write module");
    let main = dir.join("importer.qn");
    std::fs::write(&main, "<< \"broken_lib.qn\"\n^ = () -> Num => < 0 >\n").expect("write main");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("check")
        .arg(&main)
        .output()
        .expect("run quilon");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "the program must be rejected");
    assert!(
        stderr.contains(&position(&module, 2, 1)),
        "the error must be reported against the imported module, got: {stderr:?}"
    );
    assert!(
        stderr.contains(">> broken = (n :: Num) -> Text => < n >"),
        "the error must show the imported module's own source line, got: {stderr:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A function's body is a `< >` block, always. A bare expression after `=>` is a parse
/// error that says what to write instead — the one thing the reader needs.
#[test]
fn a_function_body_that_is_not_a_block_is_rejected() {
    let (ok, stderr) = check("bare_function_body", "^ = () -> Num => 42\n");

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains("a function body must be a `< >` block — write `=> < … >`"),
        "the error must name the rule and the fix, got: {stderr:?}"
    );
}

/// The same for a method inside a record or sum type.
#[test]
fn a_method_body_that_is_not_a_block_is_rejected() {
    let (ok, stderr) = check(
        "bare_method_body",
        "C = { v :: Num, get = () -> Num => it.v }\n^ = () -> Num => < 0 >\n",
    );

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains("a method body must be a `< >` block — write `=> < … >`"),
        "the error must name the rule and the fix, got: {stderr:?}"
    );
}

/// A LAMBDA is the exception: its body may still be a bare expression, which is what keeps
/// a callback on one line.
#[test]
fn a_lambda_body_may_still_be_a_bare_expression() {
    let (ok, stderr) = check(
        "bare_lambda_body",
        "^ = () -> Num => <\n  [1, 2, 3].map(x => x * 2).reduce(0, (acc, x) => acc + x)\n>\n",
    );

    assert!(
        ok,
        "a lambda's bare body must still check, stderr was: {stderr}"
    );
}

/// `quilon explain` prints the reference section for a code, and says so for a code the
/// registry lacks.
#[test]
fn explain_prints_the_section_for_a_code() {
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["explain", "QN311"])
        .output()
        .expect("run quilon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.starts_with("### QN311 — no matching overload"),
        "{stdout}"
    );
    assert!(
        stdout.contains("```"),
        "the section shows an example: {stdout}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["explain", "QN999"])
        .output()
        .expect("run quilon");
    assert_eq!(out.status.code(), Some(2));
}
