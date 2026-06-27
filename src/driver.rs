//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::diagnostic::{self, Severity};
use crate::lexer::Span;
use crate::{ast, lexer, modules, parser, typechecker};

/// A failure from any stage of the front-end. Its `Display` is the exact
/// diagnostic the CLI prints to stderr before exiting: for stages that know a
/// source location (`lex`, `parse`, `type`) it is a rustc-style
/// `path:line:col: error: …` report with the offending source line and a caret;
/// for location-less failures (`read`, `import`) it is a one-line message.
pub struct FrontEndError {
    /// The diagnostic, fully rendered against the source at construction time.
    rendered: String,
}

impl std::fmt::Display for FrontEndError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rendered)
    }
}

impl FrontEndError {
    /// A source-located error: render it rustc-style with the caret context.
    fn at(path: &str, source: &str, span: &Span, message: &str) -> Self {
        Self {
            rendered: diagnostic::render(path, source, span, Severity::Error, message),
        }
    }

    /// An error with no source location (file read failure, import resolution).
    fn plain(message: String) -> Self {
        Self { rendered: message }
    }
}

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`, returning the import-linked, checked program.
pub fn front_end(file: &Path) -> Result<ast::Program, FrontEndError> {
    let path = file.display().to_string();

    let source = std::fs::read_to_string(file)
        .map_err(|e| FrontEndError::plain(format!("error reading {}: {}", path, e)))?;

    let tokens = lexer::Lexer::tokenize(&source)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    let program = parser::parse(&tokens)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let program = modules::link(program, base_dir).map_err(FrontEndError::plain)?;

    typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|e| FrontEndError::at(&path, &source, e.span(), &e.to_string()))?;

    Ok(program)
}

/// Whether `program` defines the `^` entry point required to build an executable.
pub fn has_entry_point(program: &ast::Program) -> bool {
    program
        .items
        .iter()
        .any(|item| matches!(item, ast::Item::FunctionDecl(func) if func.name == "^"))
}
