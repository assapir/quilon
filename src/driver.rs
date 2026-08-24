//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::diagnostic::{self, Severity};
use crate::lexer::Span;
use crate::source_map::SourceMap;
use crate::{ast, lexer, modules, parser, typechecker};
use std::rc::Rc;

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

    /// A source-located error whose span may belong to an IMPORTED module, resolved through
    /// the [`SourceMap`] so it is reported against the file it is actually in.
    ///
    /// Type checking runs over the linked program, so a span can point into any module that
    /// was merged in. Rendering all of them against the root file's text named the wrong
    /// file and underlined whatever happened to sit at that byte offset in the root — a
    /// diagnostic pointing at innocent code. Falls back to the root file for a span whose
    /// file is unknown (an in-memory program), which is where the compiler is looking anyway.
    fn at_span(sources: &SourceMap, span: &Span, message: &str) -> Self {
        let Some(location) = sources.locate_or_root(span) else {
            return Self::plain(message.to_string());
        };
        let source = sources
            .get_text(span.file)
            .unwrap_or_else(|| sources.root_text());
        Self::at(&location.path, source, span, message)
    }

    /// An error with no source location (file read failure, import resolution).
    fn plain(message: String) -> Self {
        Self { rendered: message }
    }
}

/// A program that has passed the front end, together with what the later stages need
/// from that pass.
///
/// `types` is the whole point of returning a struct: the type checker computes an
/// inferred type for every expression, and codegen needs them to lower reads at their
/// declared type. Recomputing that table means type-checking the program a second time,
/// which is what this carries it here to avoid.
pub struct Checked {
    /// The import-linked, type-checked program.
    pub program: ast::Program,
    /// Every expression's inferred type, keyed by source position.
    pub types: typechecker::TypeTable,
    /// Every file the program was assembled from — the source read from `file` plus each
    /// resolved `<<` module — so any span (in the user's file or in an imported one) maps
    /// back to a path, line, and column. Shared (`Rc`) because codegen keeps it for the
    /// whole emission while the caller may still read the root text from it.
    pub sources: Rc<SourceMap>,
    /// How many leading items came from `<<` imports — `link` prepends them, so anything
    /// before this index belongs to another file. A `--debug` build uses it to attribute
    /// DWARF line info to the user's own source only.
    pub imported_items: usize,
    /// The deferred-value coloring: which expressions evaluate to a deferred (promise)
    /// value, and whether any `@` primitive launch is reachable. Codegen reads it to emit
    /// the promise representation and forces; empty for pure programs.
    pub defer: crate::deferral::DeferInfo,
}

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`.
pub fn front_end(file: &Path) -> Result<Checked, FrontEndError> {
    let path = file.display().to_string();

    let source = std::fs::read_to_string(file)
        .map_err(|e| FrontEndError::plain(format!("error reading {}: {}", path, e)))?;

    let tokens = lexer::Lexer::tokenize(&source)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    let mut program = parser::parse(&tokens)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    // Checking a corelib file directly (`quilon check corelib/io.ql`) is legitimate, and
    // its declarations are the corelib's own wherever they are read from — including the
    // inert placeholders for compiler-provided names, which are ignored on that basis.
    if modules::is_corelib_source(&source) {
        for item in &mut program.items {
            if let ast::Item::FunctionDeclaration(declaration) = item {
                declaration.from_corelib = true;
            }
        }
    }

    // The `@` marker names a leaf IO primitive, which only the corelib/runtime may
    // define; user code merely *calls* one. Reject an `@`-prefixed declaration in the
    // program's own source with a source-located diagnostic (a bare parse error would be
    // cryptic). Checked before `link` so only the user's items are scanned, never a
    // built-in module's — and skipped entirely when the file IS a corelib source (checking
    // `corelib/io.ql`/`corelib/time.ql` directly is legitimate; the corelib is the one place
    // `@` primitives are declared).
    if !modules::is_corelib_source(&source)
        && let Some((span, name)) = first_at_declaration(&program)
    {
        return Err(FrontEndError::at(
            &path,
            &source,
            span,
            &format!(
                "`{name}` cannot be declared here: `@` marks a built-in IO primitive \
                 (like `@sleep` from core.time), which only the corelib defines — user \
                 code calls one, it does not declare one"
            ),
        ));
    }

    // The source file's own item count, captured before linking prepends imported items.
    let own_item_count = program.items.len();
    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let (program, mut sources) = modules::link(program, base_dir).map_err(FrontEndError::plain)?;
    sources.set_root(path.clone(), source.clone());
    // `link` prepends imported items, so everything before the source's own items is imported.
    let imported_items = program.items.len() - own_item_count;

    let types = typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|e| FrontEndError::at_span(&sources, e.span(), &e.to_string()))?;

    // Deferred-value analysis (post-typecheck, pre-codegen): whether an `@` primitive is
    // reached, and the taint / force-set for value-returning primitives. Reads no types and
    // adds none, so the check above is unaffected. (A call's own location is not part of this:
    // codegen reads it from the source map, the same way every other located report does.)
    let defer = crate::deferral::analyze(&program);

    Ok(Checked {
        program,
        types,
        sources: Rc::new(sources),
        imported_items,
        defer,
    })
}

/// The span and name of the first top-level declaration whose name starts with `@`, if
/// any. Used to reject a user-written `@` primitive declaration (they are corelib-only).
fn first_at_declaration(program: &ast::Program) -> Option<(&Span, &str)> {
    program.items.iter().find_map(|item| match item {
        ast::Item::FunctionDeclaration(d) if d.name.starts_with('@') => {
            Some((&d.span, d.name.as_str()))
        }
        ast::Item::VariableDeclaration(d) if d.name.starts_with('@') => {
            Some((&d.span, d.name.as_str()))
        }
        _ => None,
    })
}

/// Whether `program` defines the `^` entry point required to build an executable.
pub fn has_entry_point(program: &ast::Program) -> bool {
    program
        .items
        .iter()
        .any(|item| matches!(item, ast::Item::FunctionDeclaration(func) if func.name == "^"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn corelib_file(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("corelib")
            .join(name)
    }

    /// Write `source` to a unique temp `.ql` file and return its path.
    fn temp_ql(source: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "quilon_at_decl_{}_{}.ql",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        path.push(unique);
        std::fs::write(&path, source).expect("write temp .ql");
        path
    }

    #[test]
    fn corelib_source_may_declare_at_primitives() {
        // Checking a corelib file DIRECTLY is legitimate — it is the one place `@` primitives
        // are declared, so the front-end must not reject its own `@sleep` / `@readStdin`.
        assert!(
            front_end(&corelib_file("time.ql")).is_ok(),
            "corelib core.time should check clean"
        );
        assert!(
            front_end(&corelib_file("io.ql")).is_ok(),
            "corelib core.io should check clean"
        );
    }

    #[test]
    fn user_source_may_not_declare_an_at_primitive() {
        let path = temp_ql("@bad = () -> Num => 0\n^ = () -> Num => 0\n");
        let result = front_end(&path);
        let _ = std::fs::remove_file(&path);
        match result {
            Ok(_) => panic!("a user `@` declaration must be rejected"),
            Err(error) => assert!(
                error.to_string().contains("cannot be declared here"),
                "unexpected diagnostic: {error}"
            ),
        }
    }
}
