//! Quilon's source file extension.
//!
//! Sources are `.qn`. The language previously used `.ql`, which is CodeQL's — GitHub
//! attributed Quilon programs to CodeQL because of it — and that spelling is simply not
//! Quilon any more: a file the compiler is handed has to be named for what it is.

/// Quilon's source extension.
pub const EXTENSION: &str = ".qn";

/// Accept `path` as a Quilon source, or say why it is not one. Applies to a program named
/// on the command line and to a `<<`-imported module alike — one rule, so a file that
/// cannot be a program cannot sneak in as an import either.
pub fn require_source(path: &str) -> Result<(), String> {
    if path.ends_with(EXTENSION) {
        return Ok(());
    }
    Err(format!(
        "`{path}` is not a Quilon source: sources are named `{EXTENSION}`"
    ))
}
