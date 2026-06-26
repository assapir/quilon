//! Integration tests for the `<<` module/import system (Workstream B1).

use quilon::ast::Program;
use quilon::lexer::Lexer;
use quilon::modules;
use quilon::parser::parse;
use quilon::typechecker::TypeChecker;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Lex + parse + link imports (relative to `base_dir`) + type-check. Returns the result.
fn check_with_base(source: &str, base_dir: &Path) -> Result<(), String> {
    let tokens = Lexer::tokenize(source).map_err(|e| format!("lex: {}", e))?;
    let program: Program = parse(&tokens).map_err(|e| format!("parse: {}", e))?;
    let linked = modules::link(program, base_dir)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&linked)
        .map_err(|e| format!("type: {}", e))
}

#[test]
fn test_builtin_import_resolves_and_exports_usable() {
    // `core.io` exports `print`; using it should type-check.
    let source = r#"
        << core.io
        ^ = () -> Num => print(5)
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_core_text_import_resolves() {
    // `<< core.text` must resolve; Text ops (`+`, `.size`, `.length`) are built-in.
    let source = r#"
        << core.text
        ^ = () -> Num => ("a" + "b").length
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_print_accepts_text() {
    // print is polymorphic over Num/Text; printing a Text must type-check.
    let source = r#"
        << core.io
        ^ = () -> Num => print("hello, " + "world")
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_file_path_import_exported_item_usable() {
    let source = r#"
        << "mathlib.ql"
        ^ = () -> Num => add(2, 3)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_non_exported_name_is_not_visible() {
    // `secret` exists in mathlib.ql but is not exported -> must NOT be visible.
    let source = r#"
        << "mathlib.ql"
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
        << "does_not_exist.ql"
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
