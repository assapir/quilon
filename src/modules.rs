//! Module loader for Quilon's `<<` import system.
//!
//! Resolves a program's `<< ...` imports and returns every imported module's items —
//! qualified under the module's name (see [`crate::ast::qualify`]) — to be merged into
//! the program before type checking and code generation.
//!
//! Resolution:
//! - `<< core.io` resolves to bundled built-in module source (embedded via `include_str!`).
//! - `<< "path/to.qn"` reads a user module from disk (relative to the importing file, or
//!   absolute); `\` is normalised to `/` for cross-platform paths.
//!
//! An import binds the module under its last path segment (`<< core.http` binds `http`;
//! a file import binds its file stem), and the importer reaches the module's
//! `>>`-exported items through that binding: `http.send(...)`. Non-exported items travel
//! with their module — an exported function's private helpers work — but resolve for no
//! importer: referencing one surfaces as "not exported".

use crate::ast::qualify::{self, ModuleScope, QualifyError};
use crate::ast::{Expression, Import, Item, ModulePath, Program, TypeDefinition};
use crate::lexer::{FileId, Lexer, ROOT_FILE, Span};
use crate::parser;
use crate::source_map::SourceMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A failure anywhere in import resolution or qualified-name resolution: what went wrong,
/// where (the span may point into an imported module), and every module source loaded
/// before the failure — so the caller renders the diagnostic against the file it is
/// actually in. The root file is the caller's to record.
#[derive(Debug)]
pub struct LinkError {
    pub span: Span,
    pub message: String,
    pub sources: SourceMap,
}

/// Resolve `program`'s imports and return a new program with the imported modules'
/// qualified items prepended to its own (imports cleared, since they are resolved), the
/// program's own qualified references (`http.send`) resolved against what it imported,
/// plus the [`SourceMap`] of the modules everything came from.
///
/// `root_path` is the real file the program was read from, if any (a program built from
/// an in-memory source, as most tests do, has none). When given, its canonicalized path
/// seeds the resolver's cycle guard, so a module that imports back the entry file itself
/// is caught as an import cycle rather than loaded a second time.
pub fn link(
    program: Program,
    base_dir: &Path,
    root_path: Option<&Path>,
) -> Result<(Program, SourceMap), LinkError> {
    let mut loader = Loader::new();
    if let Some(root) = root_path {
        let key = format!("file:{}", canonical_key(root).to_string_lossy());
        let label = root
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "the program".to_string());
        loader.in_progress.insert(key);
        loader.stack.push(label);
    }
    let mut program = program;
    let linked = loader
        .root_scope(&program.imports, base_dir)
        .and_then(|scope| qualify::resolve_program(&mut program, &scope))
        .and_then(|()| loader.inject_core_text(&program, base_dir));
    match linked {
        Ok(()) => {
            let mut items = loader.out;
            items.extend(program.items);
            Ok((
                Program {
                    imports: Vec::new(),
                    items,
                    // An imported module's own test blocks are that module's to run, so
                    // only the root program's survive the link.
                    test_blocks: program.test_blocks,
                },
                loader.sources,
            ))
        }
        Err(error) => Err(LinkError {
            span: error.span,
            message: error.message,
            sources: loader.sources,
        }),
    }
}

struct Loader {
    /// Canonical module name -> the resolution key it was first loaded under
    /// (`builtin:core.io` / `file:/abs/path.qn`, the latter canonicalized). Detects two
    /// DIFFERENT modules claiming one canonical name (two file imports both named
    /// `util.qn`), which the whole-program rename cannot allow; also what makes a diamond
    /// import (the same module reached twice, however spelled) resolve to one load.
    loaded: HashMap<String, String>,
    /// Resolution keys currently on the import DFS stack — importing one of these again,
    /// directly or transitively, is a cycle. Seeded with the root program's own key (see
    /// [`link`]) so a cycle back to the entry file is caught the same way.
    in_progress: HashSet<String>,
    /// Canonical names of the modules on the current DFS stack, in order — purely for a
    /// cycle diagnostic that names the chain.
    stack: Vec<String>,
    /// Canonical module name -> its exported bare names, for building importer scopes.
    exports: HashMap<String, HashSet<String>>,
    out: Vec<Item>,
    /// Each loaded module's display path and text, keyed by the `FileId` its spans carry —
    /// what a `file:line:column` for a span in an imported module is resolved through.
    sources: SourceMap,
    /// The file id the next module lexed gets. Every module's byte offsets restart at 0,
    /// so spans stay unique across the merged program only if each module carries its own
    /// identity: the type oracle is keyed by span, and a collision there hands codegen
    /// one module's inferred type for another module's expression.
    next_file: FileId,
}

impl Loader {
    fn new() -> Self {
        Self {
            loaded: HashMap::new(),
            in_progress: HashSet::new(),
            stack: Vec::new(),
            exports: HashMap::new(),
            out: Vec::new(),
            sources: SourceMap::default(),
            next_file: ROOT_FILE + 1,
        }
    }

    /// Resolve `imports` (the root program's), returning the scope the root resolves its
    /// qualified names against.
    fn root_scope(
        &mut self,
        imports: &[Import],
        base_dir: &Path,
    ) -> Result<ModuleScope, QualifyError> {
        let mut scope = ModuleScope::default();
        for import in imports {
            let canonical = self.resolve_one(import, base_dir)?;
            self.bind(&mut scope, &canonical, import)?;
        }
        Ok(scope)
    }

    /// Add one resolved import to `scope` under its short binding.
    fn bind(
        &self,
        scope: &mut ModuleScope,
        canonical: &str,
        import: &Import,
    ) -> Result<(), QualifyError> {
        let alias = crate::ast::display_name(canonical);
        let exports = self.exports.get(canonical).cloned().unwrap_or_default();
        scope.add_import(alias, canonical, exports, &import.span)
    }

    /// Load one module (and, transitively, its own imports), qualify its items under its
    /// canonical name, and append them to `out`. Returns the canonical name.
    /// Merge `core.text` when the program (or anything merged into it) calls a composable
    /// Text method — that is where those methods are implemented, and `Text` being a
    /// built-in type, no `<<` names it. The module is loaded WITHOUT binding a name in
    /// the root's scope: codegen reaches its items by their qualified names, and user
    /// code does not reach them at all unless it writes `<< core.text` itself.
    /// Reachability prunes it all back out of a program whose mention turns out not to
    /// be a Text's.
    fn inject_core_text(&mut self, program: &Program, base_dir: &Path) -> Result<(), QualifyError> {
        let uses = uses_text_composable_items(&program.items)
            || program.test_blocks.iter().any(mentions_text_composable)
            || uses_text_composable_items(&self.out);
        if !uses {
            return Ok(());
        }
        let import = Import {
            path: ModulePath::BuiltinDotted(vec!["core".to_string(), "text".to_string()]),
            span: Span::in_root(0, 0),
        };
        self.resolve_one(&import, base_dir).map(|_| ())
    }

    fn resolve_one(&mut self, import: &Import, base_dir: &Path) -> Result<String, QualifyError> {
        let from_corelib = matches!(import.path, ModulePath::BuiltinDotted(_));
        let (key, canonical, display, source, next_base) = match &import.path {
            ModulePath::BuiltinDotted(parts) => {
                let name = parts.join(".");
                let src = builtin_source(&name).ok_or_else(|| {
                    fail(&import.span, format!("unknown built-in module `{}`", name))
                })?;
                (
                    format!("builtin:{}", name),
                    name.clone(),
                    name,
                    src.to_string(),
                    base_dir.to_path_buf(),
                )
            }
            ModulePath::FilePath(raw) => {
                let normalized = raw.replace('\\', "/");
                let p = Path::new(&normalized);
                let full = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    base_dir.join(p)
                };
                // An imported module never reaches the CLI front end, so the source-name
                // rule is applied here too.
                crate::source_extension::require_source(&full.to_string_lossy())
                    .map_err(|message| fail(&import.span, message))?;
                let stem = module_binding_name(&import.path, &import.span)?;
                let source = std::fs::read_to_string(&full).map_err(|e| {
                    fail(
                        &import.span,
                        format!("cannot read module `{}`: {}", full.display(), e),
                    )
                })?;
                // Canonicalize so the same file reached through two different spellings
                // (`./sub/../sub/mod.qn` vs `sub/mod.qn`, a symlink, ...) dedups to one
                // module instead of two. The read above already proved `full` exists, so
                // canonicalization failing here is only a hedge — fall back to the
                // as-joined path rather than fail a load that just succeeded.
                let canonical_full = canonical_key(&full);
                let next_base = canonical_full
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base_dir.to_path_buf());
                (
                    format!("file:{}", canonical_full.to_string_lossy()),
                    stem,
                    full.to_string_lossy().into_owned(),
                    source,
                    next_base,
                )
            }
        };

        // Cycle guard: a module still on the DFS stack (this one included, via the root
        // seed) being imported again — directly or transitively — is an import cycle, not
        // a module to silently skip or duplicate.
        if self.in_progress.contains(&key) {
            let mut chain = self.stack.clone();
            chain.push(canonical.clone());
            return Err(fail(
                &import.span,
                format!(
                    "import cycle: {} — a module cannot import itself, directly or \
                     through others",
                    chain.join(" -> ")
                ),
            ));
        }

        // Duplicate guard: a module already fully loaded is not loaded again (a diamond
        // import, however spelled, merges into one) — but one canonical name may only
        // ever mean one module, since the whole linked program shares a single qualified
        // namespace.
        if let Some(existing_key) = self.loaded.get(&canonical) {
            if *existing_key == key {
                return Ok(canonical);
            }
            return Err(fail(
                &import.span,
                format!(
                    "two different modules are both named `{canonical}` — every imported \
                     module needs a distinct name; rename one of the files"
                ),
            ));
        }
        self.loaded.insert(canonical.clone(), key.clone());
        self.in_progress.insert(key.clone());
        self.stack.push(canonical.clone());

        let file = self.next_file;
        self.next_file += 1;
        self.sources.insert(file, display, source.clone());
        let tokens = Lexer::tokenize_in_file(&source, file)
            .map_err(|e| fail(&e.span, format!("in module `{canonical}`: {}", e.message)))?;
        let mut sub = parser::parse(&tokens)
            .map_err(|e| fail(&e.span, format!("in module `{canonical}`: {}", e.message)))?;

        // Resolve the module's own imports first (transitive), building the scope ITS
        // qualified references resolve against — a module reaches only what it imported.
        let mut scope = ModuleScope::default();
        for sub_import in &sub.imports {
            let child = self.resolve_one(sub_import, &next_base)?;
            self.bind(&mut scope, &child, sub_import)?;
        }

        self.exports
            .insert(canonical.clone(), exported_names(&sub.items));
        qualify::qualify_module(&mut sub, &canonical, &scope)?;

        for mut item in sub.items {
            // A bundled module's functions carry their origin: it is what marks the
            // corelib's inert declaration of a compiler-provided name (`print`, `write`,
            // `now`) as the placeholder it is, rather than a definition.
            if let Item::FunctionDeclaration(declaration) = &mut item {
                declaration.from_corelib = from_corelib;
            }
            self.out.push(item);
        }
        // Done resolving this module (and everything it imports) — it leaves the DFS
        // stack, so a later diamond import of it is a plain, already-loaded lookup rather
        // than a cycle.
        self.in_progress.remove(&key);
        self.stack.pop();
        Ok(canonical)
    }
}

/// Canonicalize `path` for use as a module resolution key. Falls back to `path` itself if
/// canonicalization fails (a nonexistent file) — the caller either already proved the file
/// exists (a successful read) or is about to try reading it, which fails with the ordinary
/// "cannot read module" error instead of this silently swallowing the problem.
fn canonical_key(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The bare names a module's items export: `>>`-marked functions, constants, and types —
/// plus the variants of an exported sum, which an importer reaches the same qualified way
/// (`http.Get`). `@` primitives are global once their module is imported, so they are not
/// part of the qualified surface.
fn exported_names(items: &[Item]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in items {
        if !item_is_exported(item) || item.name().starts_with('@') {
            continue;
        }
        names.insert(item.name().to_string());
        if let Item::TypeDeclaration(declaration) = item
            && let TypeDefinition::Sum { variants, .. } = &declaration.type_definition
        {
            for variant in variants {
                names.insert(variant.name.clone());
            }
        }
    }
    names
}

/// A located link failure — the shape [`qualify`]'s own errors already have.
fn fail(span: &Span, message: String) -> QualifyError {
    QualifyError {
        span: span.clone(),
        message,
    }
}

/// The short name a file-path import binds — [`ModulePath::binding_name`]'s answer, which
/// must additionally be usable as an identifier since call sites spell it
/// (`math.add(...)`).
fn module_binding_name(path: &ModulePath, span: &Span) -> Result<String, QualifyError> {
    let stem = path.binding_name().unwrap_or_default();
    let valid = !stem.is_empty()
        && !stem.starts_with(|c: char| c.is_ascii_digit())
        && stem.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !valid {
        return Err(fail(
            span,
            format!(
                "the module file name `{stem}` is not usable as a binding — call sites \
                 reach the module as `{stem}.<name>`, so the stem must be a valid \
                 identifier; rename the file"
            ),
        ));
    }
    Ok(stem)
}

// The bundled corelib module sources, embedded at compile time. Named once here so both the
// import resolver (`builtin_source`) and the trusted-origin check (`is_corelib_source`) draw
// from the same strings — they cannot drift.
const CORE_IO: &str = include_str!("../corelib/io.qn");
const CORE_TEXT: &str = include_str!("../corelib/text.qn");
const CORE_TEST: &str = include_str!("../corelib/test.qn");
const CORE_CLI: &str = include_str!("../corelib/cli.qn");
const CORE_TIME: &str = include_str!("../corelib/time.qn");
const CORE_NET: &str = include_str!("../corelib/net.qn");
const CORE_HTTP: &str = include_str!("../corelib/http.qn");
const CORE_INFO: &str = include_str!("../corelib/info.qn");

/// Every bundled corelib source — the ONE trusted origin allowed to declare `@` leaf IO
/// primitives.
const CORELIB_SOURCES: &[&str] = &[
    CORE_IO, CORE_TEXT, CORE_TEST, CORE_CLI, CORE_TIME, CORE_NET, CORE_HTTP, CORE_INFO,
];

/// Map a built-in dotted module name to its bundled source.
fn builtin_source(name: &str) -> Option<&'static str> {
    match name {
        "core.io" => Some(CORE_IO),
        // core.text — the self-hosted composable Text methods (`split`/`trim`/`contains`/
        // `replace`/`replaceAll`/`repeat`), written in Quilon over the native grapheme
        // primitives. No program imports it by name: [`link`] merges it implicitly into any
        // program that calls one of those methods, so `Text` still needs no import.
        "core.text" => Some(CORE_TEXT),
        // core.test — the test harness: `describe`/`it`, the report they print, `failAt` for
        // a check of your own, the run's recorded state, and the case lifecycle. Depends
        // transitively on core.io (it prints, and `failAt` renders its frame via `io.eprint`).
        "core.test" => Some(CORE_TEST),
        // core.cli — thin, pure-Quilon helpers over the `^` entry point's
        // `args :: []Text` and `env :: [|Text => Text|]`.
        "core.cli" => Some(CORE_CLI),
        // core.time — time-related built-ins: the deferring `@sleep` leaf primitive and
        // `time.now`, both compiler-lowered (their bodies are inert placeholders).
        "core.time" => Some(CORE_TIME),
        // core.net — the request-exchange socket primitive (`@tcpRequest`), the foundation the
        // HTTP client sits on. Like `@sleep`/`@readStdin` it is compiler-lowered (its body is
        // inert), so the import merges the primitive's signature and declares intent, nothing
        // more.
        "core.net" => Some(CORE_NET),
        // core.http — a minimal HTTP client written entirely in Quilon over core.net's
        // `@tcpRequest`. It declares no leaf IO primitives of its own; it is bundled so
        // `<< core.http` resolves and so the module is trusted when checked directly.
        "core.http" => Some(CORE_HTTP),
        // core.info — compile-time facts about the build: target CPU, target OS, and the
        // compiler's version. Like `now`, the members are compiler-provided and the module
        // body is inert; unlike `now`, each lowers to a constant rather than a runtime call.
        "core.info" => Some(CORE_INFO),
        _ => None,
    }
}

/// Whether `source` is verbatim one of the bundled corelib modules. The corelib is the one
/// place allowed to DECLARE `@` leaf IO primitives; the front-end uses this to trust a file
/// it is asked to check directly (e.g. `quilon check corelib/time.qn`) while still rejecting
/// an `@` declaration in ordinary user code. Matching by content, not path, identifies the
/// real corelib no matter where it is checked from and never mistakes user code for it.
pub fn is_corelib_source(source: &str) -> bool {
    CORELIB_SOURCES.contains(&source)
}

fn item_is_exported(item: &Item) -> bool {
    match item {
        Item::VariableDeclaration(d) => d.exported,
        Item::FunctionDeclaration(d) => d.exported,
        Item::TypeDeclaration(d) => d.exported,
    }
}

/// Whether any of `items`' bodies contains a member call to a composable Text method
/// (one [`crate::ast::qn_text_impl`] names an implementation for). Syntactic and
/// over-approximating — the receiver's type is unknown here, so `record.trim()` counts —
/// which only ever merges `core.text` needlessly, never leaves it out.
fn uses_text_composable_items(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::FunctionDeclaration(declaration) => mentions_text_composable(&declaration.body),
        Item::VariableDeclaration(declaration) => mentions_text_composable(&declaration.value),
        Item::TypeDeclaration(declaration) => declaration
            .type_definition
            .methods()
            .iter()
            .any(|method| mentions_text_composable(&method.body)),
    })
}

/// Whether `expression` contains a member call named after a composable Text method.
fn mentions_text_composable(expression: &Expression) -> bool {
    use std::ops::ControlFlow;
    crate::ast::walk::try_for_each_subexpression(expression, &mut |e| match e {
        Expression::Call {
            function,
            member_call: true,
            ..
        } if matches!(
            function.as_ref(),
            Expression::Identifier { name, .. } if crate::ast::qn_text_impl(name).is_some()
        ) =>
        {
            ControlFlow::Break(())
        }
        _ => ControlFlow::Continue(()),
    })
    .is_break()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corelib_sources_are_recognized_by_content() {
        // The trusted origins for `@` declarations: verbatim bundled corelib sources.
        assert!(is_corelib_source(CORE_TIME));
        assert!(is_corelib_source(CORE_IO));
        // Ordinary user code — even code that declares an `@` primitive — is not corelib.
        assert!(!is_corelib_source(
            "@bad = () -> Num => 0\n^ = () -> Num => 0\n"
        ));
        assert!(!is_corelib_source(""));
    }
}
