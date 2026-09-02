//! Shared compiler front-end.
//!
//! The `check`, `compile`, and `run` commands all begin the same way: read the
//! source file, lex it, parse it, resolve its `<<` imports, and type-check the
//! result. This module owns that pipeline so the commands only differ in their
//! tails (print a summary, emit LLVM IR, or JIT-execute).

use std::path::Path;

use crate::diagnostic::{Code, Diagnostic};
use crate::lexer::{SYNTHESIZED_FILE, Span};
use crate::source_map::SourceMap;
use crate::status::{Stage, Status};
use crate::{ast, lexer, modules, parser, typechecker};
use std::rc::Rc;

/// A failure from any stage of the front end: the structured [`Diagnostic`] — code,
/// message, byte spans into the files in `sources` — which is what a language server
/// consumes, and which `Display` renders as the report the CLI prints.
///
/// The type checker runs over the linked program, so a span can point into any module
/// that was merged in; `sources` holds every file loaded before the failure, so the report
/// names the file the span is actually in rather than underlining whatever sat at that byte
/// offset in the root.
#[derive(Debug)]
pub struct FrontEndError {
    pub diagnostic: Diagnostic,
    pub sources: SourceMap,
}

impl std::fmt::Display for FrontEndError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(false))
    }
}

impl FrontEndError {
    /// The report, colored or plain.
    pub fn render(&self, color: bool) -> String {
        self.diagnostic.render(&self.sources, color)
    }

    /// An error in the root file, before any module was linked.
    fn in_root(path: &str, source: &str, diagnostic: Diagnostic) -> Self {
        let mut sources = SourceMap::default();
        sources.set_root(path, source);
        Self {
            diagnostic,
            sources,
        }
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
/// [`ast::TEST_BLOCK_MARKER`]).
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

/// The function the synthesized test entry point ends with: it renders the run's summary and
/// yields the run's status. Bound by NAME in the linked program's scope, where `core.test`'s
/// definition answers.
const SUMMARY_FUNCTION: &str = "core.test.reportSummary";

/// The module carrying the harness, named in the diagnostic a suite gets when the summary
/// function is not in scope at all.
const HARNESS_MODULE: &str = "core.test";

/// Read, lex, parse, resolve `<<` imports (relative to `file`'s directory), and
/// type-check the program at `file`, leaving its test blocks out (see [`TestBlocks`]).
pub fn front_end(file: &Path) -> Result<Checked, FrontEndError> {
    front_end_with(file, TestBlocks::Erase)
}

/// [`front_end`], choosing what happens to the file's top-level `describe` blocks.
pub fn front_end_with(file: &Path, tests: TestBlocks) -> Result<Checked, FrontEndError> {
    front_end_reporting(file, tests, &Status::silent())
}

/// [`front_end_with`], announcing each stage through `status` as it begins.
pub fn front_end_reporting(
    file: &Path,
    tests: TestBlocks,
    status: &Status,
) -> Result<Checked, FrontEndError> {
    let path = file.display().to_string();
    let unlocated = |code, message| FrontEndError {
        diagnostic: Diagnostic::new(code, message),
        sources: SourceMap::default(),
    };
    crate::source_extension::require_source(&path)
        .map_err(|message| unlocated(Code::NotAQuilonSource, message))?;

    let source = std::fs::read_to_string(file).map_err(|e| {
        unlocated(
            Code::SourceNotReadable,
            format!("cannot read `{path}`: {e}"),
        )
    })?;

    status.stage(Stage::Lexing);
    let tokens = lexer::Lexer::tokenize(&source).map_err(|e| {
        FrontEndError::in_root(&path, &source, Diagnostic::at(e.code, &e.span, e.message))
    })?;

    status.stage(Stage::Parsing);
    let mut program = parser::parse(&tokens).map_err(|e| {
        let mut diagnostic = Diagnostic::at(e.code, &e.span, e.message);
        diagnostic.help = e.help;
        FrontEndError::in_root(&path, &source, diagnostic)
    })?;

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
        return Err(FrontEndError::in_root(
            &path,
            &source,
            Diagnostic::at(
                Code::AtDeclarationOutsideCorelib,
                span,
                format!(
                    "`{name}` cannot be declared here: `@` marks a built-in IO primitive \
                     (like `@sleep` from core.time), which only the corelib defines"
                ),
            )
            .help("user code calls a primitive; it does not declare one"),
        ));
    }

    let tests_only = !program.test_blocks.is_empty() && !has_entry_point(&program);

    status.stage(Stage::Resolving);
    let base_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let (mut program, mut sources) = match modules::link(program, base_dir, Some(file)) {
        Ok(linked) => linked,
        // A link failure's span may point into an imported module; the error carries the
        // sources loaded so far, so the diagnostic renders against the right file.
        Err(mut error) => {
            error.sources.set_root(path.clone(), source.clone());
            return Err(FrontEndError {
                diagnostic: Diagnostic::at(error.code, &error.span, error.message),
                sources: error.sources,
            });
        }
    };
    sources.set_root(path.clone(), source.clone());

    if tests == TestBlocks::Run
        && let Err(diagnostic) = synthesize_test_entry(&mut program)
    {
        return Err(FrontEndError {
            diagnostic,
            sources,
        });
    }

    status.stage(Stage::Checking);
    let types = match typechecker::TypeChecker::new().check_program(&program) {
        Ok(types) => types,
        Err(error) => {
            return Err(FrontEndError {
                diagnostic: error.diagnostic(),
                sources,
            });
        }
    };

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
    SummaryName,
    SummaryCall,
    EntryBody,
    EntryDeclaration,
}

fn synthesized_span(node: Synthesized) -> Span {
    let offset = node as u32;
    Span::in_file(offset, offset, SYNTHESIZED_FILE)
}

/// Append the entry point that runs `program`'s test blocks: each `describe(…)` in source
/// order, then the run's summary, whose `Num` result becomes the exit code.
///
/// A file's tests may sit beside its code, `^` included, so the program reaching here may
/// already have something under that name. Whatever it is, it belongs to the program and not
/// to the test run, and only one thing can carry the name: it is dropped, because the entry
/// appended below is the entry point now.
fn synthesize_test_entry(program: &mut ast::Program) -> Result<(), Diagnostic> {
    let Some(first_block) = program.test_blocks.first() else {
        return Ok(());
    };
    program.items.retain(|item| !names_entry_point(item));
    // The summary function has to be in scope, since the entry ends by calling it. Said here,
    // at the first test block, rather than by the type checker at the synthesized call — which
    // has no source location to point a diagnostic at.
    if !defines_function(program, SUMMARY_FUNCTION) {
        return Err(Diagnostic::at(
            Code::NoTestHarness,
            first_block.span(),
            format!("no test harness in scope: `{SUMMARY_FUNCTION}` is undefined"),
        )
        .help(format!("add `<< {HARNESS_MODULE}` above this block")));
    }

    let summary = ast::Expression::Call {
        function: Box::new(ast::Expression::Identifier {
            name: SUMMARY_FUNCTION.to_string(),
            span: synthesized_span(Synthesized::SummaryName),
        }),
        arguments: Vec::new(),
        member_call: false,
        span: synthesized_span(Synthesized::SummaryCall),
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
            binding_type: None,
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
        let path = temp_source("@bad = () -> Num => < 0 >\n^ = () -> Num => < 0 >\n");
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
