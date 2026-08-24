//! `.qn` is Quilon's source extension; `.ql` still compiles for this release.
//!
//! A `.ql` program must build and run exactly as it did, saying it is deprecated on stderr
//! and nowhere else: a warning that failed the build, or that landed on stdout, would break
//! the pipelines that read a program's output. All of this goes away at 1.0 with the
//! extension itself — except the last test, which outlives it.

mod common;

use common::{run_file, run_program_named};
use quilon::source_extension::{EXTENSION, LEGACY_EXTENSION};
use std::process::Command;

const PROGRAM: &str = r#"
<< core.io

^ = () -> Num => <
  "out" |> write(stdout)
  7
>
"#;

#[test]
fn a_current_program_runs_with_nothing_on_stderr() {
    let run = run_program_named(&format!("modern{EXTENSION}"), PROGRAM);
    assert_eq!(run.code, 7);
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "", "the current extension warns about nothing");
}

#[test]
fn a_legacy_program_still_runs_and_says_it_is_deprecated() {
    let run = run_program_named(&format!("legacy{LEGACY_EXTENSION}"), PROGRAM);
    assert_eq!(run.code, 7, "`.ql` still compiles this release");
    assert_eq!(run.stdout, "out", "the warning does not reach stdout");
    assert!(
        run.stderr.contains("`.ql` is deprecated")
            && run.stderr.contains("1.0")
            && run.stderr.contains("legacy.qn"),
        "the warning must name the extension, when it stops working, and the new file \
         name; got: {}",
        run.stderr
    );
}

#[test]
fn a_legacy_module_import_is_named_too() {
    // The deprecation is about files, not about the program's own path: a `<<` import of a
    // legacy module is named, under its own name, even from a current program.
    let root = run_program_named(
        &format!("root{EXTENSION}"),
        "<< \"helper.ql\"\n\n^ = () -> Num => <\n  answer()\n>\n",
    )
    .path;
    let directory = root.parent().expect("the program's directory");
    std::fs::write(directory.join("helper.ql"), ">> answer = () -> Num => 7\n")
        .expect("writing the imported module");

    let run = run_file(&root);
    assert_eq!(run.code, 7, "the import still resolves");
    assert!(
        run.stderr.contains("helper.qn"),
        "the imported module is named in the warning; got: {}",
        run.stderr
    );
}

#[test]
fn no_tracked_source_uses_the_legacy_extension() {
    // What the language bar counts is TRACKED files, so that is what this asks about — and
    // asking git rather than walking keeps sibling worktrees, build output, and a
    // developer's scratch files out of the answer. This one outlives the deprecation: a
    // stray `.ql` would be attributed to CodeQL, which is the whole reason for the rename.
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
        "these tracked sources still use the deprecated extension:\n{tracked}"
    );
}
