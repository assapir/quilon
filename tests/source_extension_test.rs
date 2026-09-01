//! Quilon sources are `.qn`, and nothing else is a Quilon source.
//!
//! `.ql` — CodeQL's extension, which Quilon used until 0.9.1 — is not accepted: a file the
//! compiler is handed has to be named for what it is, or the rename would only have moved
//! the misattribution rather than ended it. The rejection is by name, before the file is
//! read, and it applies to a `<<`-imported module as well as to the program itself.

mod common;

use common::{run_file, run_program_named};
use quilon::source_extension::EXTENSION;
use std::process::Command;

const PROGRAM: &str = r#"
<< core.io

^ = () -> Num => <
  io.write("out", io.stdout)
  7
>
"#;

#[test]
fn a_qn_program_runs() {
    let run = run_program_named(&format!("modern{EXTENSION}"), PROGRAM);
    assert_eq!(run.code, 7);
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "", "a source named correctly says nothing");
}

#[test]
fn a_ql_program_is_rejected_by_name() {
    let run = run_program_named("legacy.ql", PROGRAM);
    assert_ne!(run.code, 7, "a `.ql` file is not a Quilon source");
    assert_eq!(run.stdout, "", "it is rejected before it is compiled");
    assert!(
        run.stderr.contains("legacy.ql") && run.stderr.contains(EXTENSION),
        "the error must name the file and the extension sources use; got: {}",
        run.stderr
    );
}

#[test]
fn a_source_with_no_extension_is_rejected_too() {
    // The rule is what a source is *named*, not what it isn't: a bare name is no more a
    // Quilon source than a `.ql` one, and gets the same answer.
    let run = run_program_named("program", PROGRAM);
    assert_ne!(run.code, 7);
    assert!(
        run.stderr.contains("program") && run.stderr.contains(EXTENSION),
        "got: {}",
        run.stderr
    );
}

#[test]
fn a_ql_module_import_is_rejected_under_its_own_name() {
    let root = run_program_named(
        &format!("root{EXTENSION}"),
        "<< \"helper.ql\"\n\n^ = () -> Num => <\n  answer()\n>\n",
    )
    .path;
    let directory = root.parent().expect("the program's directory");
    std::fs::write(
        directory.join("helper.ql"),
        ">> answer = () -> Num => < 7 >\n",
    )
    .expect("writing the imported module");

    let run = run_file(&root);
    assert_ne!(run.code, 7, "the import is not resolved");
    assert!(
        run.stderr.contains("helper.ql"),
        "the rejected module is named, not just the program importing it; got: {}",
        run.stderr
    );
}

#[test]
fn no_tracked_source_uses_the_old_extension() {
    // What the language bar counts is TRACKED files, so that is what this asks about — and
    // asking git rather than walking keeps sibling worktrees, build output, and a
    // developer's scratch files out of the answer. A stray `.ql` would be attributed to
    // CodeQL, which is the whole reason for the rename.
    let listed = Command::new("git")
        .args(["ls-files", "--", "*.ql"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();
    let Ok(listed) = listed else {
        eprintln!("skipping: git is not on PATH");
        return;
    };
    if !listed.status.success() {
        eprintln!("skipping: not a git checkout");
        return;
    }
    let tracked = String::from_utf8_lossy(&listed.stdout);
    assert!(
        tracked.trim().is_empty(),
        "these tracked sources still use the old extension:\n{tracked}"
    );
}
