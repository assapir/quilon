//! DWARF line-number debug information (Phase 1).
//!
//! When a native build is requested with `--debug`, the code generator installs a
//! [`DebugInfo`] alongside the LLVM module. It owns the `DebugInfoBuilder`, the compile
//! unit / source file handles, and a line-start index that turns a byte offset (every AST
//! node carries a `Span` of byte offsets) into a 1-based `(line, column)`.
//!
//! Scope is deliberately narrow: a `DISubprogram` per emitted function plus per-instruction
//! source locations, which is exactly what a debugger needs to map a program counter back to
//! a `.ql` line and to step through source. Local-variable and full-type debug info is a
//! later phase — the subroutine type here is intentionally empty (no parameter/return types).

use std::path::Path;

use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DIFlags, DIFlagsConstants, DILocation, DIScope, DISubprogram,
    DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};

use crate::codegen::generator::WATERMARK;
use crate::lexer::Span;

/// DWARF debug-info state for one module, created only under `--debug`.
pub struct DebugInfo<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    compile_unit: DICompileUnit<'ctx>,
    file: DIFile<'ctx>,
    /// Byte offset at which each source line begins (`line_starts[0] == 0`). Used to find
    /// a span's line (and its line-start byte) by binary search.
    line_starts: Vec<usize>,
    /// The source text, kept so a span's DWARF column can be counted in characters (not
    /// bytes) from its line start — matching how the compiler's diagnostics report columns.
    source: String,
}

impl<'ctx> DebugInfo<'ctx> {
    /// Install debug-info emission on `module` for the program compiled from `file_path`
    /// with the given `source`. Adds the required module flags and creates the compile
    /// unit + source-file handles.
    pub fn new(
        module: &Module<'ctx>,
        context: &'ctx Context,
        file_path: &Path,
        source: &str,
    ) -> Self {
        // The verifier requires a "Debug Info Version" module flag; consumers key off it.
        let dwarf_version = context.i32_type().const_int(4, false);
        let debug_info_version = context.i32_type().const_int(3, false);
        module.add_basic_value_flag("Dwarf Version", FlagBehavior::Warning, dwarf_version);
        module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Warning,
            debug_info_version,
        );

        // Split the path into a directory + filename for the DIFile. Both are recorded in
        // the line table so a debugger can locate the `.ql` source.
        let directory = file_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        let filename = file_path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.display().to_string());

        let (builder, compile_unit) = module.create_debug_info_builder(
            true,
            // The closest DWARF source-language tag; Quilon has no dedicated code, and C
            // is the conventional choice for a small native language front-end.
            DWARFSourceLanguage::C,
            &filename,
            &directory,
            WATERMARK,
            // `--debug` builds at -O0, so nothing is optimized.
            false,
            "",
            0,
            "",
            // Full (not LineTablesOnly) so the compile unit + subprograms are emitted.
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );
        let file = compile_unit.get_file();

        DebugInfo {
            builder,
            compile_unit,
            file,
            line_starts: line_starts(source),
            source: source.to_string(),
        }
    }

    /// The 1-based `(line, column)` of `offset` in the source. The column counts characters
    /// (not bytes) from the line start, so multi-byte characters before `offset` advance it
    /// by one each — matching the compiler's diagnostics. A byte offset past the end clamps
    /// to the last line (defensive; spans always point inside the source).
    fn line_col(&self, offset: usize) -> (u32, u32) {
        // Index of the last line start that is <= offset.
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line = idx + 1;
        let line_start = self.line_starts[idx];
        let end = offset.min(self.source.len());
        // Count characters from the line start; fall back to the byte delta if `offset` is
        // not on a char boundary (spans are token starts, so this is only a guard).
        let col = self
            .source
            .get(line_start..end)
            .map(|s| s.chars().count())
            .unwrap_or(end - line_start)
            + 1;
        (line as u32, col as u32)
    }

    /// Create a `DISubprogram` for a function named `name` beginning at `span`, to be
    /// attached to its `FunctionValue` and used as the scope for its instructions.
    pub fn create_function(&self, name: &str, span: &Span) -> DISubprogram<'ctx> {
        let (line, _) = self.line_col(span.start as usize);
        // Line-tables phase: an empty subroutine type (no parameter/return types).
        let subroutine_type =
            self.builder
                .create_subroutine_type(self.file, None, &[], DIFlags::PUBLIC);
        self.builder.create_function(
            self.compile_unit.as_debug_info_scope(),
            name,
            None,
            self.file,
            line,
            subroutine_type,
            true,
            true,
            line,
            DIFlags::PUBLIC,
            false,
        )
    }

    /// A debug location at `span`'s start within `scope`, for `set_current_debug_location`.
    pub fn location(
        &self,
        context: &'ctx Context,
        span: &Span,
        scope: DIScope<'ctx>,
    ) -> DILocation<'ctx> {
        let (line, col) = self.line_col(span.start as usize);
        self.builder
            .create_debug_location(context, line, col, scope, None)
    }

    /// Resolve all forward references and finalize the debug metadata. Must run before the
    /// module is verified or emitted to an object file.
    pub fn finalize(&self) {
        self.builder.finalize();
    }
}

/// The byte offset at which each line begins. `line_starts[0]` is always `0`; a new entry
/// follows every `\n`. Lines are 1-based, so line `n` starts at `line_starts[n - 1]`.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}
