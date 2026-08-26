//! End-to-end diagnostics gate: drives the real `quilon` binary on deliberately
//! broken programs and asserts the `path:line:col:` position line, the `error: …` message
//! line under it,
//! (with the offending source line and a caret underline) reaches stderr, and
//! that the process still exits non-zero.

use std::io::Write;
use std::process::Command;

mod common;
use common::position;

/// Write `source` to a temp `.qn` file, run `quilon check` on it, and return
/// `(exit_success, stderr)`. The file lives under the cargo target tmp dir so
/// parallel test runs don't collide.
fn check(name: &str, source: &str) -> (bool, String) {
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
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn type_error_reports_line_col_and_caret() {
    // `a + true` has no `+` overload for (Num, Bool) — a clear no-match error on line 2.
    let src = "~ comment\nadd = (a :: Num) -> Num => a + true\n^ = () -> Num => add(1)\n";
    let (ok, stderr) = check("type", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    // The position line, with the message on the line under it.
    assert!(
        stderr.contains(":2:28:\nerror:"),
        "no line:col position line: {stderr}"
    );
    // `+` is now a visible overload set; a Num/Bool mix matches no member.
    assert!(
        stderr.contains("No overload of '+'"),
        "no message: {stderr}"
    );
    // The offending source line is echoed...
    assert!(
        stderr.contains("add = (a :: Num) -> Num => a + true"),
        "source line missing: {stderr}"
    );
    // ...with a caret underline beneath it.
    assert!(stderr.contains('^'), "no caret underline: {stderr}");
}

#[test]
fn lexer_error_reports_line_col_and_caret() {
    // `#` is not a valid token. (`@` used to be invalid, but now marks a deferring
    // primitive like `@sleep`, so it is a real token.)
    let src = "^ = () -> Num => #\n";
    let (ok, stderr) = check("lex", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    assert!(
        stderr.contains(":1:18:\nerror:"),
        "no line:col position line: {stderr}"
    );
    assert!(stderr.contains("Invalid token"), "no message: {stderr}");
    assert!(stderr.contains('^'), "no caret underline: {stderr}");
}

#[test]
fn parse_error_reports_line_col() {
    // A function with no body after `=>` is a parse error.
    let src = "^ = () -> Num =>\n";
    let (ok, stderr) = check("parse", src);

    assert!(!ok, "expected non-zero exit, stderr was: {stderr}");
    // The position line + message shape holds for parse failures too.
    assert!(
        position_line_followed_by_error(&stderr),
        "no `:line:col:` position line with an `error:` line under it: {stderr}"
    );
}

/// Whether `stderr` has a `…:<line>:<col>:` position line immediately followed by an
/// `error: <message>` line — the two-line header every diagnostic opens with.
fn position_line_followed_by_error(stderr: &str) -> bool {
    let lines: Vec<&str> = stderr.lines().collect();
    lines.windows(2).any(|pair| {
        let Some(position) = pair[0].strip_suffix(':') else {
            return false;
        };
        // The two segments at the end of the position line are the line and column.
        let mut numbers = position.rsplit(':');
        let column = numbers.next().and_then(|s| s.parse::<usize>().ok());
        let row = numbers.next().and_then(|s| s.parse::<usize>().ok());
        column.is_some() && row.is_some() && pair[1].starts_with("error: ")
    })
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
        "~ a module with a type error\n>> broken = (n :: Num) -> Text => n\n",
    )
    .expect("write module");
    let main = dir.join("importer.qn");
    std::fs::write(&main, "<< \"broken_lib.qn\"\n^ = () -> Num => 0\n").expect("write main");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("check")
        .arg(&main)
        .output()
        .expect("run quilon");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "the program must be rejected");
    assert!(
        stderr.starts_with(&format!("{}\nerror:", position(&module, 2, 1))),
        "the error must be reported against the imported module, got: {stderr:?}"
    );
    assert!(
        stderr.contains(">> broken = (n :: Num) -> Text => n"),
        "the error must show the imported module's own source line, got: {stderr:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
