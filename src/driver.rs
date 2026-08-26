//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::diagnostic::{self, Severity};
use crate::lexer::{SYNTHESIZED_FILE, Span};
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
    /// The deferred-value coloring: which expressions evaluate to a deferred (promise)
    /// value, and whether any `@` primitive launch is reachable. Codegen reads it to emit
    /// the promise representation and forces; empty for pure programs.
    pub defer: crate::deferral::DeferInfo,
}

/// What the front end does with a file's top-level `describe` blocks (see
/// [`ast::TEST_BLOCK_MARKER`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestBlocks {
    /// Leave them out of the compilation unit. `check`, `compile`, `build`, and `run` all
    /// take this path, so a file's tests are never checked, never emitted, and cannot
    /// reach a release binary.
    Strip,
    /// Compile them, under an entry point synthesized to run each block in order. What
    /// `quilon test` uses.
    Run,
}

/// The reporter function the synthesized test entry point ends with: it renders the run's
/// summary and yields the process exit code (0 when every case passed).
///
/// This is the reporter SEAM. `core.test`'s `describe`/`it`/matchers record what happened
/// through the reporter-agnostic test registry (see [`ast::TEST_REGISTRY_INTRINSICS`]) and
/// render through the `report*` functions beside this one; nothing about the harness is
/// wired to a particular rendering. Selecting another reporter is a matter of pointing
/// this name at another module's function.
pub const REPORTER_SUMMARY_FUNCTION: &str = "reportSummary";

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`, leaving its test blocks out (see [`TestBlocks`]).
pub fn front_end(file: &Path) -> Result<Checked, FrontEndError> {
    front_end_with(file, TestBlocks::Strip)
}

/// [`front_end`], choosing what happens to the file's top-level `describe` blocks.
pub fn front_end_with(file: &Path, tests: TestBlocks) -> Result<Checked, FrontEndError> {
    let path = file.display().to_string();
    crate::source_extension::require_source(&path).map_err(FrontEndError::plain)?;

    let source = std::fs::read_to_string(file)
        .map_err(|e| FrontEndError::plain(format!("error reading {}: {}", path, e)))?;

    let tokens = lexer::Lexer::tokenize(&source)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    let mut program = parser::parse(&tokens)
        .map_err(|e| FrontEndError::at(&path, &source, &e.span, &e.message))?;

    // Checking a corelib file directly (`quilon check corelib/io.qn`) is legitimate, and
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
    // `corelib/io.qn`/`corelib/time.qn` directly is legitimate; the corelib is the one place
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

    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let (mut program, mut sources) =
        modules::link(program, base_dir).map_err(FrontEndError::plain)?;
    sources.set_root(path.clone(), source.clone());

    if tests == TestBlocks::Run {
        synthesize_test_entry(&mut program)
            .map_err(|(span, message)| FrontEndError::at(&path, &source, &span, &message))?;
    }

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
    entry_point(program).is_some()
}

/// The program's `^` entry-point declaration, if it has one.
fn entry_point(program: &ast::Program) -> Option<&ast::FunctionDeclaration> {
    program.items.iter().find_map(|item| match item {
        ast::Item::FunctionDeclaration(func) if func.name == "^" => Some(func),
        _ => None,
    })
}

/// A span for the `index`-th node the compiler builds itself. Spans key the type oracle,
/// so each synthesized node needs its own — and none may collide with a node read from a
/// real source, which is what [`SYNTHESIZED_FILE`] guarantees.
fn synthesized_span(index: u32) -> Span {
    Span::in_file(index, index, SYNTHESIZED_FILE)
}

/// Append the entry point that runs `program`'s test blocks: each `describe(…)` in source
/// order, then the reporter's summary, whose `Num` result becomes the exit code.
///
/// Fails (with the offending span and message) if the file also defines its own `^` — a
/// test file has no entry point, because this is it.
fn synthesize_test_entry(program: &mut ast::Program) -> Result<(), (Span, String)> {
    if program.test_blocks.is_empty() {
        return Ok(());
    }
    if let Some(existing) = entry_point(program) {
        return Err((
            existing.span.clone(),
            format!(
                "a file with top-level `{}` blocks must not define `^`: `quilon test` \
                 synthesizes the entry point that runs them",
                ast::TEST_BLOCK_MARKER
            ),
        ));
    }

    let summary = ast::Expression::Call {
        function: Box::new(ast::Expression::Identifier {
            name: REPORTER_SUMMARY_FUNCTION.to_string(),
            span: synthesized_span(0),
        }),
        arguments: Vec::new(),
        span: synthesized_span(1),
    };
    let statements = program
        .test_blocks
        .drain(..)
        .chain(std::iter::once(summary))
        .map(ast::Statement::Expression)
        .collect();

    program
        .items
        .push(ast::Item::FunctionDeclaration(ast::FunctionDeclaration {
            name: "^".to_string(),
            parameters: Vec::new(),
            return_type: Some(ast::Type::Num),
            body: ast::Expression::Block {
                statements,
                span: synthesized_span(2),
            },
            exported: false,
            from_corelib: false,
            span: synthesized_span(3),
        }));
    Ok(())
}

/// Whether `file` holds nothing but tests: top-level `describe` blocks and imports, with
/// no declarations of its own. Such a file is not a compilation unit at all — `build`,
/// `compile`, and `run` skip it silently rather than reporting a missing `^`.
///
/// Answered from a parse alone (no import resolution, no type check), so it is cheap
/// enough to ask of every `.qn` file when discovering a directory's tests. `None` when the
/// file cannot be read or parsed, which leaves judging it to the ordinary front end.
pub fn test_blocks_only(file: &Path) -> Option<bool> {
    let program = parse_only(file)?;
    Some(!program.test_blocks.is_empty() && program.items.is_empty())
}

/// Whether `file` has any top-level test block at all. `None` when it cannot be read or
/// parsed.
pub fn has_test_blocks(file: &Path) -> Option<bool> {
    Some(!parse_only(file)?.test_blocks.is_empty())
}

/// Read and parse `file`, with no import resolution or type checking. `None` on any
/// failure: the callers are asking a question about a file's SHAPE, and a file that cannot
/// be parsed has no answer to give — the ordinary front end is what reports why.
fn parse_only(file: &Path) -> Option<ast::Program> {
    let source = std::fs::read_to_string(file).ok()?;
    let tokens = lexer::Lexer::tokenize(&source).ok()?;
    parser::parse(&tokens).ok()
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

    /// Write `source` to a unique temp `.qn` file and return its path.
    fn temp_source(source: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "quilon_at_decl_{}_{}.qn",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        path.push(unique);
        std::fs::write(&path, source).expect("write temp .qn");
        path
    }

    #[test]
    fn corelib_source_may_declare_at_primitives() {
        // Checking a corelib file DIRECTLY is legitimate — it is the one place `@` primitives
        // are declared, so the front-end must not reject its own `@sleep` / `@readStdin`.
        assert!(
            front_end(&corelib_file("time.qn")).is_ok(),
            "corelib core.time should check clean"
        );
        assert!(
            front_end(&corelib_file("io.qn")).is_ok(),
            "corelib core.io should check clean"
        );
    }

    #[test]
    fn user_source_may_not_declare_an_at_primitive() {
        let path = temp_source("@bad = () -> Num => 0\n^ = () -> Num => 0\n");
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
