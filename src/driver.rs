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
    /// The file is a test suite rather than a program: it has top-level test blocks and no
    /// `^` of its own (whatever fixtures its cases need are fine). Erasing the blocks leaves
    /// nothing to run, so `run`/`compile`/`build` pass over it in silence. Recorded before
    /// the link, which merges an `^` from nowhere but would blur "of its own".
    pub tests_only: bool,
}

/// What the front end does with a file's top-level `describe` blocks (see
/// [`ast::TEST_BLOCK_MARKER`]) — and, with them, its test-only `<<?` imports (see
/// [`ast::Import::test_only`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestBlocks {
    /// Leave them out of the compilation unit. `check`, `compile`, `build`, and `run` all
    /// take this path, so a file's tests are never checked, never emitted, and cannot
    /// reach a release binary.
    Erase,
    /// Compile them, under an entry point synthesized to run each block in order. What
    /// `quilon test` uses.
    Run,
}

/// The reporter function the synthesized test entry point ends with: it renders the run's
/// summary and yields the run's status. Bound by NAME, in the linked program's scope, so
/// the definition that answers is `core.test.report`'s when a suite imports it and the suite's
/// own when it does not — which is the whole of how a reporter is selected. See the seam in
/// `docs/corelib/test.md`.
pub const REPORTER_SUMMARY_FUNCTION: &str = "reportSummary";

/// The module carrying the reporter Quilon ships, named in the diagnostic a suite gets when
/// no reporter is in scope at all.
pub const DEFAULT_REPORTER_MODULE: &str = "core.test.report";

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`, leaving its test blocks out (see [`TestBlocks`]).
pub fn front_end(file: &Path) -> Result<Checked, FrontEndError> {
    front_end_with(file, TestBlocks::Erase)
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

    let tests_only = !program.test_blocks.is_empty() && !has_entry_point(&program);

    // Erase the test-only imports, exactly as the test blocks are erased: they serve those
    // blocks, so they follow them. Kept aside rather than dropped so a name that went missing
    // with them can be explained. `quilon test` compiles the blocks, so it keeps the imports.
    let erased_imports: Vec<ast::Import> = match tests {
        TestBlocks::Erase => {
            let (erased, kept) = std::mem::take(&mut program.imports)
                .into_iter()
                .partition(|import| import.test_only);
            program.imports = kept;
            erased
        }
        TestBlocks::Run => Vec::new(),
    };

    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let (mut program, mut sources) =
        modules::link(program, base_dir).map_err(FrontEndError::plain)?;
    sources.set_root(path.clone(), source.clone());

    if tests == TestBlocks::Run {
        synthesize_test_entry(&mut program)
            .map_err(|(span, message)| FrontEndError::at_span(&sources, &span, &message))?;
    }

    let types = typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|e| {
            let message =
                erased_import_explanation(&erased_imports, base_dir, &sources, &program, &e)
                    .unwrap_or_else(|| e.to_string());
            FrontEndError::at_span(&sources, e.span(), &message)
        })?;

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
        tests_only,
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

/// Explain an undefined name that one of the `erased_imports` would have provided, or `None`
/// when the error is about something else.
///
/// Resolving the erased modules costs a parse, so it happens here — on the failure path — and
/// nowhere else.
fn erased_import_explanation(
    erased_imports: &[ast::Import],
    base_dir: &Path,
    sources: &SourceMap,
    program: &ast::Program,
    error: &crate::typechecker::checker::TypeError,
) -> Option<String> {
    let crate::typechecker::checker::TypeError::UndefinedVariable { name, span } = error else {
        return None;
    };
    if !report_is_over_the_name(sources, span, name) {
        return None;
    }
    // A name the compilation unit DOES define is missing for another reason — Quilon resolves
    // top to bottom, so a use above the definition reads as undefined — and the erased import
    // is not the answer.
    if program.items.iter().any(|item| item.name() == name) {
        return None;
    }
    let module = modules::providing_import(erased_imports, base_dir, name)?;
    Some(format!(
        "`{name}` comes in through `<<? {module}`, a test-only import: it is resolved for \
         the `{}` blocks under `quilon test` and erased everywhere else, so nothing outside \
         a block can use it. Move this use into a block, or import the module with `<<`",
        ast::TEST_BLOCK_MARKER,
    ))
}

/// Whether `span` covers `name` itself — a use of that name — rather than a larger expression
/// that merely contains it.
///
/// `UndefinedVariable` carries two different mistakes: a name nothing defines, reported over
/// exactly that name (or over `Name { … }` for a constructor), and an unknown FIELD, reported
/// over the whole access, `point.red`. Telling them apart is what keeps a mistyped field from
/// being blamed on a test-only import that happens to export something spelled the same.
fn report_is_over_the_name(sources: &SourceMap, span: &Span, name: &str) -> bool {
    let text = sources
        .get_text(span.file)
        .unwrap_or_else(|| sources.root_text());
    let Some(quoted) = text.get(span.start as usize..span.end as usize) else {
        return false;
    };
    quoted == name
        || quoted
            .strip_prefix(name)
            .is_some_and(|rest| rest.trim_start().starts_with('{'))
}

/// Whether `program` defines the `^` entry point required to build an executable.
pub fn has_entry_point(program: &ast::Program) -> bool {
    defines_function(program, "^")
}

/// Whether `program` declares a top-level function called `name`.
fn defines_function(program: &ast::Program, name: &str) -> bool {
    program
        .items
        .iter()
        .any(|item| matches!(item, ast::Item::FunctionDeclaration(func) if func.name == name))
}

/// Whether `item` claims the `^` name — as the entry-point function, or as a top-level
/// binding that would collide with one.
fn names_entry_point(item: &ast::Item) -> bool {
    match item {
        ast::Item::FunctionDeclaration(declaration) => declaration.name == "^",
        ast::Item::VariableDeclaration(declaration) => declaration.name == "^",
        _ => false,
    }
}

/// The four nodes [`synthesize_test_entry`] builds, as the offsets that tell their spans
/// apart. Spans key the type oracle, so each synthesized node needs its own — and none may
/// collide with a node read from a real source, which is what [`SYNTHESIZED_FILE`]
/// guarantees.
#[derive(Clone, Copy)]
enum Synthesized {
    ReporterName,
    ReporterCall,
    EntryBody,
    EntryDeclaration,
}

fn synthesized_span(node: Synthesized) -> Span {
    let offset = node as u32;
    Span::in_file(offset, offset, SYNTHESIZED_FILE)
}

/// Append the entry point that runs `program`'s test blocks: each `describe(…)` in source
/// order, then the reporter's summary, whose `Num` result becomes the exit code.
///
/// A file's tests may sit beside its code, `^` included, so the program reaching here may
/// already have something under that name. Whatever it is, it belongs to the program and not
/// to the test run, and only one thing can carry the name: it is dropped, because the entry
/// appended below is the entry point now.
fn synthesize_test_entry(program: &mut ast::Program) -> Result<(), (Span, String)> {
    let Some(first_block) = program.test_blocks.first() else {
        return Ok(());
    };
    program.items.retain(|item| !names_entry_point(item));
    // The reporter has to be in scope, since the entry ends by calling it. Said here, at the
    // first test block, rather than by the type checker at the synthesized call — which has
    // no source location to point a diagnostic at.
    if !defines_function(program, REPORTER_SUMMARY_FUNCTION) {
        return Err((
            first_block.span().clone(),
            format!(
                "no test reporter in scope: `{REPORTER_SUMMARY_FUNCTION}` is undefined. \
                 Add `<< {DEFAULT_REPORTER_MODULE}` for the one Quilon ships, or define \
                 `{REPORTER_SUMMARY_FUNCTION}` yourself"
            ),
        ));
    }

    let summary = ast::Expression::Call {
        function: Box::new(ast::Expression::Identifier {
            name: REPORTER_SUMMARY_FUNCTION.to_string(),
            span: synthesized_span(Synthesized::ReporterName),
        }),
        arguments: Vec::new(),
        span: synthesized_span(Synthesized::ReporterCall),
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
                span: synthesized_span(Synthesized::EntryBody),
            },
            exported: false,
            from_corelib: false,
            span: synthesized_span(Synthesized::EntryDeclaration),
        }));
    Ok(())
}

/// Whether `file` is a suite `quilon test` should run: it has top-level test blocks — or it
/// cannot be parsed at all, in which case running it is the only safe answer. Passing over a
/// broken source in silence would let `quilon test` report success on a suite whose syntax
/// someone had just broken; running it makes the front end say what is wrong.
///
/// Answered from a parse alone — no import resolution, no type check — which is what makes it
/// cheap enough to ask of every `.qn` file under a directory. A file that cannot even be READ
/// is not a suite: there is nothing to run and nothing to report.
pub fn is_test_suite(file: &Path) -> bool {
    let Ok(source) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(tokens) = lexer::Lexer::tokenize(&source) else {
        return true;
    };
    match parser::parse(&tokens) {
        Ok(program) => !program.test_blocks.is_empty(),
        Err(_) => true,
    }
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
