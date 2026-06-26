//! Module loader for Quilon's `<<` import system (Workstream B1).
//!
//! Resolves a program's `<< ...` imports and returns the **exported** items of every
//! imported module (transitively), to be merged into the importing program's global scope
//! before type checking and code generation.
//!
//! Resolution:
//! - `<< core.io` resolves to bundled built-in module source (embedded via `include_str!`).
//! - `<< "path/to.ql"` reads a user module from disk (relative to the importing file, or
//!   absolute); `\` is normalised to `/` for cross-platform paths.
//!
//! Visibility: only items marked exported (`>>` prefix) are merged. Non-exported items are
//! module-private, so referencing them from an importer surfaces as a normal "undefined"
//! error. NOTE (minimal release): an exported item that depends on a *private* sibling item
//! is therefore not yet supported across the merge — core-lib exports instead bottom out in
//! compiler intrinsics (`__print`, …), not private `.ql` helpers.

use crate::ast::{Import, Item, ModulePath, Program};
use crate::lexer::Lexer;
use crate::parser;
use std::collections::HashSet;
use std::path::Path;

/// Resolve all imports of `program`, returning the exported items to merge into the
/// importing program. `base_dir` is the directory of the importing file (used to resolve
/// relative file-path imports).
pub fn resolve_imports(program: &Program, base_dir: &Path) -> Result<Vec<Item>, String> {
    let mut loader = Loader {
        visited: HashSet::new(),
        out: Vec::new(),
    };
    loader.resolve_list(&program.imports, base_dir)?;
    Ok(loader.out)
}

struct Loader {
    visited: HashSet<String>,
    out: Vec<Item>,
}

impl Loader {
    fn resolve_list(&mut self, imports: &[Import], base_dir: &Path) -> Result<(), String> {
        for import in imports {
            self.resolve_one(&import.path, base_dir)?;
        }
        Ok(())
    }

    fn resolve_one(&mut self, path: &ModulePath, base_dir: &Path) -> Result<(), String> {
        let (canonical, source, next_base) = match path {
            ModulePath::BuiltinDotted(parts) => {
                let name = parts.join(".");
                let src = builtin_source(&name)
                    .ok_or_else(|| format!("unknown built-in module `{}`", name))?;
                (
                    format!("builtin:{}", name),
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
                let source = std::fs::read_to_string(&full)
                    .map_err(|e| format!("cannot read module `{}`: {}", full.display(), e))?;
                let next_base = full
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base_dir.to_path_buf());
                (
                    format!("file:{}", full.to_string_lossy()),
                    source,
                    next_base,
                )
            }
        };

        // Cycle / duplicate guard: skip modules already loaded on this resolution.
        if !self.visited.insert(canonical.clone()) {
            return Ok(());
        }

        let tokens = Lexer::tokenize(&source)
            .map_err(|e| format!("lexer error in module `{}`: {}", canonical, e))?;
        let sub = parser::parse(&tokens)
            .map_err(|e| format!("parse error in module `{}`: {}", canonical, e))?;

        // Resolve the module's own imports first (transitive), then collect its exports.
        self.resolve_list(&sub.imports, &next_base)?;
        for item in sub.items {
            if item_is_exported(&item) {
                self.out.push(item);
            }
        }
        Ok(())
    }
}

/// Map a built-in dotted module name to its bundled source.
fn builtin_source(name: &str) -> Option<&'static str> {
    match name {
        "core.io" => Some(include_str!("../corelib/io.ql")),
        "core.text" => Some(include_str!("../corelib/text.ql")),
        _ => None,
    }
}

fn item_is_exported(item: &Item) -> bool {
    match item {
        Item::VarDecl(d) => d.exported,
        Item::FunctionDecl(d) => d.exported,
        Item::TypeDecl(d) => d.exported,
    }
}

/// Convenience used by the CLI: resolve `program`'s imports and return a new program with the
/// imported exported items prepended to its own items (imports cleared, since they are resolved).
pub fn link(program: Program, base_dir: &Path) -> Result<Program, String> {
    let mut items = resolve_imports(&program, base_dir)?;
    items.extend(program.items);
    Ok(Program {
        imports: Vec::new(),
        items,
    })
}
