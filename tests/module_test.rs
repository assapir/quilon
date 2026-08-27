//! Integration tests for the `<<` module/import system (Workstream B1).

use quilon::ast::{Item, Program};
use quilon::lexer::{FileId, Lexer, ROOT_FILE, Span};
use quilon::modules;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Lex + parse + link imports (relative to `base_dir`) + type-check. Returns the result.
fn check_with_base(source: &str, base_dir: &Path) -> Result<(), String> {
    let tokens = Lexer::tokenize(source).map_err(|e| format!("lex: {}", e))?;
    let program: Program = parse(&tokens).map_err(|e| format!("parse: {}", e))?;
    let (linked, _sources) = modules::link(program, base_dir)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&linked)
        .map(|_| ())
        .map_err(|e| format!("type: {}", e))
}

#[test]
fn test_builtin_import_resolves_and_exports_usable() {
    // `core.io` exports `print`; using it should type-check. `print` returns `$`.
    let source = r#"
        << core.io
        ^ = () -> $ => print(5)
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_text_ops_need_no_import() {
    // Text is a built-in primitive (like Num/arrays): its ops (`+`, `.size`,
    // `.length`) work with NO import.
    let source = r#"^ = () -> Num => ("a" + "b").length"#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_core_text_is_not_a_module() {
    // There is no `core.text` module — Text is intrinsic — so importing it errors.
    let source = r#"
        << core.text
        ^ = () -> Num => 0
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_err(), "expected unknown-module error, got ok");
}

#[test]
fn test_print_accepts_text() {
    // print is polymorphic over Num/Text; printing a Text must type-check.
    // `print` returns `$` (Unit), so the entry point is annotated `-> $`.
    let source = r#"
        << core.io
        ^ = () -> $ => print("hello, " + "world")
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_user_defined_print_not_shadowed() {
    // Regression: a user-defined `print` with its own signature must be resolved
    // normally, not hard-shadowed by the polymorphic builtin (which only accepts a
    // single Num/Text/Bool arg). A 2-arg user `print` must type-check.
    let source = r#"
        print = (a :: Num, b :: Num) -> Num => a + b
        ^ = () -> Num => print(2, 3)
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_core_test_module_resolves_and_type_checks() {
    // `<< core.test` is a registered built-in module: importing it resolves, and the
    // provided assertions type-check beside what it exports.
    let source = r#"
        << core.test
        ^ = () -> $ => <
          assert(1, equals(1))
          assert(6 * 7, equals(42))
          assert("a" + "b", equals("ab"))
          assert(1 < 2, equals(true))
          assert(1, not(equals(2)))
          assert([1, 2].at(0), isOk())
          assert([1, 2].at(9), isNotOk())
        >
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_file_path_import_exported_item_usable() {
    let source = r#"
        << "mathlib.qn"
        ^ = () -> Num => add(2, 3)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_non_exported_name_is_not_visible() {
    // `secret` exists in mathlib.qn but is not exported -> must NOT be visible.
    let source = r#"
        << "mathlib.qn"
        ^ = () -> Num => secret(3)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    assert!(
        result.is_err(),
        "expected a type error for the private `secret`, but it type-checked"
    );
}

#[test]
fn test_unknown_builtin_module_errors() {
    let source = r#"
        << core.nope
        ^ = () -> Num => 0
    "#;
    let result = check_with_base(source, Path::new("."));
    let err = result.expect_err("expected an import error for an unknown module");
    assert!(
        err.contains("unknown built-in module"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_missing_file_module_errors() {
    let source = r#"
        << "does_not_exist.qn"
        ^ = () -> Num => 0
    "#;
    let result = check_with_base(source, &fixtures_dir());
    let err = result.expect_err("expected an import error for a missing file");
    assert!(
        err.contains("cannot read module"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_program_without_imports_still_works() {
    let source = r#"
        ^ = () -> Num => 42
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_each_module_gets_its_own_file_identity() {
    // Every imported module is lexed on its own, so the loader hands each one a
    // distinct, non-root id. That id is what makes two modules' identical byte
    // ranges distinguishable downstream.
    let source = r#"
        << "mathlib.qn"
        << "span_twin.qn"
        ^ = () -> Num => add(2, 3)
    "#;
    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
    let (items, _sources) = modules::resolve_imports(&program, &fixtures_dir()).unwrap();

    let files: HashSet<FileId> = items.iter().map(|item| item_span(item).file).collect();
    assert_eq!(files.len(), 2, "one id per module, got {:?}", files);
    assert!(
        !files.contains(&ROOT_FILE),
        "no imported item may claim the root file's identity: {:?}",
        files
    );
}

fn item_span(item: &Item) -> &Span {
    match item {
        Item::VariableDeclaration(d) => &d.span,
        Item::FunctionDeclaration(d) => &d.span,
        Item::TypeDeclaration(d) => &d.span,
    }
}
