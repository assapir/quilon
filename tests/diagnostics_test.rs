//! End-to-end diagnostics gate: drives the real `quilon` binary on deliberately
//! broken programs and asserts the rustc-style `path:line:col: error: …` report
//! (with the offending source line and a caret underline) reaches stderr, and
//! that the process still exits non-zero.

use std::io::Write;
use std::process::Command;

/// Write `source` to a temp `.ql` file, run `quilon check` on it, and return
/// `(exit_success, stderr)`. The file lives under the cargo target tmp dir so
/// parallel test runs don't collide.
fn check(name: &str, source: &str) -> (bool, String) {
    let mut path = std::env::temp_dir();
    path.push(format!("quilon_diag_{}_{}.ql", std::process::id(), name));
    let mut f = std::fs::File::create(&path).expect("create temp .ql");
    f.write_all(source.as_bytes()).expect("write temp .ql");

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
    // line:column appears in the header.
    assert!(
        stderr.contains(":2:28: error:"),
        "no line:col header: {stderr}"
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
        stderr.contains(":1:18: error:"),
        "no line:col header: {stderr}"
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
    // The header follows the `:line:col: error:` shape for parse failures too.
    assert!(
        stderr.lines().any(is_line_col_error_header),
        "no `:line:col: error:` header: {stderr}"
    );
}

/// Whether `line` ends in the `…:<line>:<col>: error: <message>` header shape.
fn is_line_col_error_header(line: &str) -> bool {
    let Some((location, rest)) = line.split_once(": error: ") else {
        return false;
    };
    // The two segments immediately before `: error:` are the line and column.
    let mut nums = location.rsplit(':');
    let col = nums.next().and_then(|s| s.parse::<usize>().ok());
    let row = nums.next().and_then(|s| s.parse::<usize>().ok());
    row.is_some() && col.is_some() && !rest.is_empty()
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
    let module = dir.join("broken_lib.ql");
    std::fs::write(
        &module,
        "~ a module with a type error\n>> broken = (n :: Num) -> Text => n\n",
    )
    .expect("write module");
    let main = dir.join("importer.ql");
    std::fs::write(&main, "<< \"broken_lib.ql\"\n^ = () -> Num => 0\n").expect("write main");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("check")
        .arg(&main)
        .output()
        .expect("run quilon");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "the program must be rejected");
    assert!(
        stderr.starts_with(&format!("{}:2:1: error:", module.display())),
        "the error must be reported against the imported module, got: {stderr:?}"
    );
    assert!(
        stderr.contains(">> broken = (n :: Num) -> Text => n"),
        "the error must show the imported module's own source line, got: {stderr:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
