//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::{ast, lexer, modules, parser, typechecker};

/// A failure from any stage of the front-end. Its `Display` is the exact message
/// the CLI prints to stderr before exiting.
pub enum FrontEndError {
    Read(std::io::Error),
    Lex(String),
    Parse(String),
    Import(String),
    Type(String),
}

impl std::fmt::Display for FrontEndError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontEndError::Read(e) => write!(f, "❌ Error reading file: {}", e),
            FrontEndError::Lex(e) => write!(f, "❌ Lexer error: {}", e),
            FrontEndError::Parse(e) => write!(f, "❌ Parse error: {}", e),
            FrontEndError::Import(e) => write!(f, "❌ Import error: {}", e),
            FrontEndError::Type(e) => write!(f, "❌ Type error: {}", e),
        }
    }
}

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`, returning the import-linked, checked program.
pub fn front_end(file: &Path) -> Result<ast::Program, FrontEndError> {
    let source = std::fs::read_to_string(file).map_err(FrontEndError::Read)?;

    let tokens = lexer::Lexer::tokenize(&source).map_err(|e| FrontEndError::Lex(e.to_string()))?;

    let program = parser::parse(&tokens).map_err(|e| FrontEndError::Parse(e.to_string()))?;

    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let program =
        modules::link(program, base_dir).map_err(|e| FrontEndError::Import(e.to_string()))?;

    typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|e| FrontEndError::Type(e.to_string()))?;

    Ok(program)
}

/// Whether `program` defines the `^` entry point required to build an executable.
pub fn has_entry_point(program: &ast::Program) -> bool {
    program
        .items
        .iter()
        .any(|item| matches!(item, ast::Item::FunctionDecl(func) if func.name == "^"))
}
