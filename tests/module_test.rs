//! Integration tests for the `<<` module/import system: qualified access through the
//! module's binding, privates traveling with their module, and the import-time errors.

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
    let (linked, _sources) =
        modules::link(program, base_dir).map_err(|e| format!("link: {}", e.message))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&linked)
        .map(|_| ())
        .map_err(|e| format!("type: {}", e))
}

#[test]
fn test_builtin_import_resolves_and_exports_usable() {
    // `core.io` exports `print`, reached through the import's binding: `io.print`.
    let source = r#"
        << core.io
        ^ = () -> $ => io.print(5)
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_full_path_reaches_an_export_too() {
    // The fully-qualified spelling always works beside the short binding.
    let source = r#"
        << core.io
        ^ = () -> $ => core.io.print(5)
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_bare_export_name_is_not_in_scope() {
    // Qualified access by default: an import binds `io`, it does not merge `print`
    // into the file's namespace.
    let source = r#"
        << core.io
        ^ = () -> $ => print(5)
    "#;
    let result = check_with_base(source, Path::new("."));
    let err = result.expect_err("a bare `print` must not resolve under `<< core.io`");
    assert!(err.contains("print"), "unexpected error: {}", err);
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
    // print takes any renderable value; printing a Text must type-check.
    // `print` returns `$` (Unit), so the entry point is annotated `-> $`.
    let source = r#"
        << core.io
        ^ = () -> $ => io.print("hello, " + "world")
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_user_print_is_an_ordinary_function() {
    // A module's overload sets are closed: a program's own `print` is simply an
    // unrelated function — defined and called bare, at any signature.
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
    // A file import binds its stem: `"mathlib.qn"` is reached as `mathlib.<name>`.
    let source = r#"
        << "mathlib.qn"
        ^ = () -> Num => mathlib.add(2, 3)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_non_exported_name_is_not_visible() {
    // `secret` exists in mathlib.qn but is not exported -> must NOT resolve, and the
    // error must not distinguish "private" from "nonexistent".
    let source = r#"
        << "mathlib.qn"
        ^ = () -> Num => mathlib.secret(3)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    let err = result.expect_err("the private `secret` must not resolve for an importer");
    assert!(err.contains("not exported"), "unexpected error: {}", err);
}

#[test]
fn test_private_sibling_travels_with_its_module() {
    // An exported function may lean on a private sibling: the private item is carried
    // through the link (under its qualified name), even though no importer can name it.
    let source = r#"
        << "with_helper.qn"
        ^ = () -> Num => with_helper.quad(4)
    "#;
    let result = check_with_base(source, &fixtures_dir());
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn test_import_claims_its_short_name() {
    // After `<< core.io`, a binding named `io` — top-level or local — is rejected.
    let source = r#"
        << core.io
        ^ = () -> Num => <
          io = 5
          io
        >
    "#;
    let err = check_with_base(source, Path::new("."))
        .expect_err("a local binding may not reuse an import's short name");
    assert!(err.contains("claimed"), "unexpected error: {}", err);
}

#[test]
fn test_module_binding_is_not_a_value() {
    // The binding reaches the module's exports; it is not itself a value.
    let source = r#"
        << core.io
        ^ = () -> $ => io.print(io)
    "#;
    let err = check_with_base(source, Path::new("."))
        .expect_err("a module binding must not pass as a value");
    assert!(err.contains("not a value"), "unexpected error: {}", err);
}

#[test]
fn test_import_binds_only_the_code_below_it() {
    // Like every name — the language has no hoisting — an import qualifies only what
    // is written below it.
    let source = r#"
        early = () -> $ => io.print(5)
        << core.io
        ^ = () -> $ => early()
    "#;
    let result = check_with_base(source, Path::new("."));
    assert!(
        result.is_err(),
        "an `io.print` above `<< core.io` must not resolve"
    );
}

#[test]
fn test_a_stem_colliding_with_a_builtin_binding_is_rejected() {
    // `core.test` binds `test`, and a file module named `test.qn` has ONLY that name —
    // no full path to fall back on — so the collision errors at the import.
    let source = r#"
        << core.test
        << "test.qn"
        ^ = () -> Num => 0
    "#;
    let err = check_with_base(source, &fixtures_dir())
        .expect_err("a file stem colliding with a bound short name must be rejected");
    assert!(err.contains("rename the file"), "unexpected error: {}", err);
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
        ^ = () -> Num => mathlib.add(2, 3)
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
