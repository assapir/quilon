//! Module loader for Quilon's `<<` import system (Workstream B1).
//!
//! Resolves a program's `<< ...` imports and returns the **exported** items of every
//! imported module (transitively), to be merged into the importing program's global scope
//! before type checking and code generation.
//!
//! Resolution:
//! - `<< core.io` resolves to bundled built-in module source (embedded via `include_str!`).
//! - `<< "path/to.qn"` reads a user module from disk (relative to the importing file, or
//!   absolute); `\` is normalised to `/` for cross-platform paths.
//!
//! Visibility: only items marked exported (`>>` prefix) are merged. Non-exported items are
//! module-private, so referencing them from an importer surfaces as a normal "undefined"
//! error. NOTE (minimal release): an exported item that depends on a *private* sibling item
//! is therefore not yet supported across the merge — core-lib exports instead bottom out in
//! compiler intrinsics (`__print`, …), not private `.qn` helpers.

use crate::ast::{Import, Item, ModulePath, Program};
use crate::lexer::{FileId, Lexer, ROOT_FILE};
use crate::parser;
use crate::source_map::SourceMap;
use std::collections::HashSet;
use std::path::Path;

/// Resolve all imports of `program`, returning the exported items to merge into the
/// importing program, and a [`SourceMap`] naming every module those items came from (the
/// root file is the caller's to record). `base_dir` is the directory of the importing file
/// (used to resolve relative file-path imports).
pub fn resolve_imports(
    program: &Program,
    base_dir: &Path,
) -> Result<(Vec<Item>, SourceMap), String> {
    let mut loader = Loader {
        visited: HashSet::new(),
        out: Vec::new(),
        sources: SourceMap::default(),
        next_file: ROOT_FILE + 1,
    };
    loader.resolve_list(&program.imports, base_dir)?;
    Ok((loader.out, loader.sources))
}

struct Loader {
    visited: HashSet<String>,
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
    fn resolve_list(&mut self, imports: &[Import], base_dir: &Path) -> Result<(), String> {
        for import in imports {
            self.resolve_one(&import.path, base_dir)?;
        }
        Ok(())
    }

    fn resolve_one(&mut self, path: &ModulePath, base_dir: &Path) -> Result<(), String> {
        let from_corelib = matches!(path, ModulePath::BuiltinDotted(_));
        let (canonical, display, source, next_base) = match path {
            ModulePath::BuiltinDotted(parts) => {
                let name = parts.join(".");
                let src = builtin_source(&name)
                    .ok_or_else(|| format!("unknown built-in module `{}`", name))?;
                (
                    format!("builtin:{}", name),
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
                crate::source_extension::require_source(&full.to_string_lossy())?;
                let source = std::fs::read_to_string(&full)
                    .map_err(|e| format!("cannot read module `{}`: {}", full.display(), e))?;
                let next_base = full
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base_dir.to_path_buf());
                (
                    format!("file:{}", full.to_string_lossy()),
                    full.to_string_lossy().into_owned(),
                    source,
                    next_base,
                )
            }
        };

        // Cycle / duplicate guard: skip modules already loaded on this resolution.
        if !self.visited.insert(canonical.clone()) {
            return Ok(());
        }

        let file = self.next_file;
        self.next_file += 1;
        self.sources.insert(file, display, source.clone());
        let tokens = Lexer::tokenize_in_file(&source, file)
            .map_err(|e| format!("lexer error in module `{}`: {}", canonical, e))?;
        let sub = parser::parse(&tokens)
            .map_err(|e| format!("parse error in module `{}`: {}", canonical, e))?;

        // Resolve the module's own imports first (transitive), then collect its exports.
        self.resolve_list(&sub.imports, &next_base)?;
        for mut item in sub.items {
            if item_is_exported(&item) {
                // A bundled module's functions carry their origin: it is what marks the
                // corelib's inert declaration of a compiler-provided name (`print`,
                // `write`, `now`) as the placeholder it is, rather than a definition.
                if let Item::FunctionDeclaration(declaration) = &mut item {
                    declaration.from_corelib = from_corelib;
                }
                self.out.push(item);
            }
        }
        Ok(())
    }
}

// The bundled corelib module sources, embedded at compile time. Named once here so both the
// import resolver (`builtin_source`) and the trusted-origin check (`is_corelib_source`) draw
// from the same strings — they cannot drift.
const CORE_IO: &str = include_str!("../corelib/io.qn");
const CORE_TEST: &str = include_str!("../corelib/test.qn");
const CORE_CLI: &str = include_str!("../corelib/cli.qn");
const CORE_TIME: &str = include_str!("../corelib/time.qn");
const CORE_NET: &str = include_str!("../corelib/net.qn");

/// Every bundled corelib source — the ONE trusted origin allowed to declare `@` leaf IO
/// primitives.
const CORELIB_SOURCES: &[&str] = &[CORE_IO, CORE_TEST, CORE_CLI, CORE_TIME, CORE_NET];

/// Map a built-in dotted module name to its bundled source.
fn builtin_source(name: &str) -> Option<&'static str> {
    match name {
        "core.io" => Some(CORE_IO),
        // core.test — assertions (`assert` + wrappers) for self-verifying programs.
        // Depends transitively on core.io (its wrappers render values via `eprint`).
        "core.test" => Some(CORE_TEST),
        // core.cli — thin, pure-Quilon helpers over the `^` entry point's
        // `args :: []Text` and `env :: [|Text => Text|]`.
        "core.cli" => Some(CORE_CLI),
        // core.time — time-related leaf IO primitives (`@sleep`). Documentation-only:
        // `@sleep` is a compiler-provided built-in (lowered to the runtime scheduler),
        // like `print`/`write`, so importing the module makes intent explicit but merges
        // no items. It is the documented home of the deferring `@sleep` primitive.
        "core.time" => Some(CORE_TIME),
        // core.net — the request-exchange socket primitive (`@tcpRequest`), the foundation the
        // HTTP client sits on. Like `@sleep`/`@readStdin` it is compiler-lowered (its body is
        // inert), so the import merges the primitive's signature and declares intent, nothing
        // more.
        "core.net" => Some(CORE_NET),
        // Text is a built-in primitive type (like Num/Bool/arrays): its operations
        // (`+`, `.size`, `.length`) are compiler-intrinsic and need no import, so
        // there is intentionally no `core.text` module.
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

/// Convenience used by the CLI: resolve `program`'s imports and return a new program with the
/// imported exported items prepended to its own items (imports cleared, since they are resolved),
/// plus the [`SourceMap`] of the modules they came from.
pub fn link(program: Program, base_dir: &Path) -> Result<(Program, SourceMap), String> {
    let (mut items, sources) = resolve_imports(&program, base_dir)?;
    items.extend(program.items);
    Ok((
        Program {
            imports: Vec::new(),
            items,
        },
        sources,
    ))
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
