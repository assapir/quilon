//! Quilon's source file extension, and the transition off the one it used to have.
//!
//! A leaf module: the CLI front end and the module loader both name the extension, and
//! neither owns it. When `.ql` support is dropped at 1.0, everything the compiler knows
//! about the old extension is in this file.

/// Quilon's source extension.
pub const EXTENSION: &str = ".qn";

/// The extension Quilon shipped with, and CodeQL's — which is why it is going. Still
/// compiles, with the warning below; the support goes away at 1.0.
pub const LEGACY_EXTENSION: &str = ".ql";

/// Warn, on stderr, that `path` uses the deprecated extension — once per source file read,
/// and never fatal: a `.ql` program still compiles and runs exactly as it did. Deliberately
/// outside the [`crate::diagnostic`] renderer, which reports a span within a source; this is
/// about the file's *name*, so it has no line, column, or excerpt to point at.
pub fn warn_if_legacy(path: &str) {
    if let Some(stem) = path.strip_suffix(LEGACY_EXTENSION) {
        eprintln!(
            "warning: `{LEGACY_EXTENSION}` is deprecated as Quilon's source extension and \
             stops working at 1.0 — rename `{path}` to `{stem}{EXTENSION}`"
        );
    }
}
