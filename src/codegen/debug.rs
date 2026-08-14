//! DWARF debug information.
//!
//! When a native build is requested with `--debug`, the code generator installs a
//! [`DebugInfo`] alongside the LLVM module. It owns the `DebugInfoBuilder`, the compile
//! unit / source file handles, and a line-start index that turns a byte offset (every AST
//! node carries a `Span` of byte offsets) into a 1-based `(line, column)`.
//!
//! The line table comes first: a `DISubprogram` per function plus per-instruction source
//! locations, which a debugger needs to map a program counter back to a `.ql` line.
//!
//! On top of that this module emits **debug types and local variables**. The DWARF-type
//! builders below map the Quilon type system onto distinct DWARF entries: `Num`/`Bool` are
//! basic types, while `Text`, `[]T`, records, and sum types — which all lower to the SAME
//! `{ptr, i64}`-ish LLVM shape — get DISTINCT named composite types so a debugger (and a
//! future pretty-printer) can tell a `Text` from a `[]Num` from a `User` record from a
//! `Result`. Parameters and `=`/`:=` locals then get a `DILocalVariable` (typed via these
//! builders) plus an `llvm.dbg.declare`, attached to their function's subprogram or a nested
//! lexical block.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DIFlags, DIFlagsConstants, DILocalVariable, DILocation,
    DIScope, DISubprogram, DIType, DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};
use inkwell::values::PointerValue;

use crate::codegen::generator::WATERMARK;
use crate::lexer::Span;

// DWARF `DW_ATE_*` base-type encodings (see the DWARF spec, table 7.11). Passed to
// `create_basic_type` so a debugger renders each primitive with the right interpretation.
const DW_ATE_BOOLEAN: u32 = 0x02;
const DW_ATE_FLOAT: u32 = 0x04;
const DW_ATE_SIGNED: u32 = 0x05;
const DW_ATE_SIGNED_CHAR: u32 = 0x06;
const DW_ATE_UNSIGNED: u32 = 0x07;

/// Round `value` up to the next multiple of `align` (both in bits). `align == 0` is treated
/// as `1` so the result is always defined.
fn align_up(value: u64, align: u64) -> u64 {
    let align = align.max(1);
    value.div_ceil(align) * align
}

/// The natural alignment (in bits) of a struct member. `create_basic_type` records NO
/// alignment (it has no such parameter), so `get_align_in_bits()` is `0` for our scalars —
/// using it directly would collapse every member to byte alignment and place a wide field
/// (an `f64` payload after an `i8` tag) at the wrong offset. Instead derive it: a composite
/// (a pointer, or a nested struct) already carries a real alignment, so honor it; a scalar
/// aligns to its own size, capped at 64 bits — matching x86-64's default data layout where a
/// `double`/`i64`/pointer aligns to 8 bytes and an `i8` to 1.
fn member_align_bits(ty: DIType<'_>) -> u32 {
    let recorded = ty.get_align_in_bits();
    if recorded > 0 {
        return recorded;
    }
    let size = ty.get_size_in_bits();
    (size.clamp(8, 64)) as u32
}

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
    /// Cache of built DWARF composite/derived types, keyed by a structural name (e.g.
    /// `"Text"`, `"[]Num"`, `"named$User"`, `"sum$Result"`). A type identity is emitted once
    /// and shared by every variable of that type, so `llvm-dwarfdump` shows one entry per
    /// distinct Quilon type rather than a fresh struct per variable. `RefCell` because the
    /// builders take `&self` (they run from codegen's read-only emission helpers).
    type_cache: RefCell<HashMap<String, DIType<'ctx>>>,
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
            type_cache: RefCell::new(HashMap::new()),
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
        // Line info only: an empty subroutine type (no parameter/return types).
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

    // ---- DWARF debug types ----------------------------------------------------------
    //
    // Each Quilon type maps to a distinct DWARF entry. The primitives (`Num`, `Bool`) are
    // basic types; `Text`, `[]T`, records and sum types share a `{ptr, i64}`-ish LLVM shape
    // but are given DISTINCT named composite types so a debugger can tell them apart.
    // Composite types are cached by a structural key so each identity is emitted once.

    /// A previously built DWARF type for `key`, if any. Callers check this before building
    /// a composite so a repeated type (e.g. two `[]Num` locals) shares one DWARF entry.
    pub fn cached_type(&self, key: &str) -> Option<DIType<'ctx>> {
        self.type_cache.borrow().get(key).copied()
    }

    /// Record `ty` under `key` for later reuse by [`cached_type`].
    pub fn cache_type(&self, key: &str, ty: DIType<'ctx>) {
        self.type_cache.borrow_mut().insert(key.to_string(), ty);
    }

    /// `Num` — an IEEE-754 double (`f64`), the language's single numeric type.
    pub fn num_type(&self) -> DIType<'ctx> {
        self.basic_type("Num", 64, DW_ATE_FLOAT)
    }

    /// `Bool` — a one-byte boolean (`i1`, stored in a byte).
    pub fn bool_type(&self) -> DIType<'ctx> {
        self.basic_type("Bool", 8, DW_ATE_BOOLEAN)
    }

    /// `$` (Unit) — the one-inhabitant type, lowered as a zero byte.
    pub fn unit_type(&self) -> DIType<'ctx> {
        self.basic_type("$", 8, DW_ATE_UNSIGNED)
    }

    fn basic_type(&self, name: &str, size_in_bits: u64, encoding: u32) -> DIType<'ctx> {
        self.builder
            .create_basic_type(name, size_in_bits, encoding, DIFlags::PUBLIC)
            .expect("basic type name is non-empty")
            .as_type()
    }

    /// An opaque pointer (`i8*`), used for a type codegen can't model precisely (a bare
    /// function value, or a recursive type broken to stop infinite metadata).
    pub fn opaque_pointer(&self) -> DIType<'ctx> {
        let byte = self.basic_type("", 8, DW_ATE_SIGNED_CHAR);
        self.pointer_to("", byte)
    }

    /// A 64-bit pointer to `pointee`, optionally named.
    pub fn pointer_to(&self, name: &str, pointee: DIType<'ctx>) -> DIType<'ctx> {
        self.builder
            .create_pointer_type(name, pointee, 64, 64, AddressSpace::default())
            .as_type()
    }

    /// `Text` — a `{ ptr data, i64 byte_len }` struct over a NUL-terminated UTF-8 buffer.
    /// Distinct from an array by name (`Text`) and by its `data` pointee (`char`, not `T`).
    pub fn text_type(&self) -> DIType<'ctx> {
        let char_ty = self.basic_type("char", 8, DW_ATE_SIGNED_CHAR);
        let data = self.pointer_to("", char_ty);
        let byte_len = self.basic_type("i64", 64, DW_ATE_SIGNED);
        self.struct_type("Text", &[("data", data), ("byte_len", byte_len)])
    }

    /// `[]T` — a `{ ptr data, i64 size }` struct whose `data` points at `elem`-typed
    /// elements. `name` carries the element type (e.g. `"[]Num"`), and the pointee is the
    /// element's own DWARF type, so it is distinct from both `Text` and a different `[]U`.
    pub fn array_type(&self, name: &str, elem: DIType<'ctx>) -> DIType<'ctx> {
        let data = self.pointer_to("", elem);
        let size = self.basic_type("i64", 64, DW_ATE_SIGNED);
        self.struct_type(name, &[("data", data), ("size", size)])
    }

    /// A record type: a `DICompositeType` struct of its named fields, returned as a POINTER
    /// to that struct — Quilon's record ABI passes and stores records by pointer, so the
    /// pointer is what actually sits in the variable's slot. Distinct from every other type
    /// by its name and by its members.
    pub fn record_type(&self, name: &str, fields: &[(String, DIType<'ctx>)]) -> DIType<'ctx> {
        let members: Vec<(&str, DIType<'ctx>)> =
            fields.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        let strukt = self.struct_type(name, &members);
        self.pointer_to(name, strukt)
    }

    /// A sum type: a `{ i8 tag, payload... }` tagged-union struct. The tag discriminates the
    /// active variant; the payload slots mirror codegen's canonical layout (one slot per
    /// payload position). Distinct from records/arrays/`Text` by its name and its leading tag.
    ///
    /// This tagged-struct is layout-faithful — the tag and every payload slot land at the same
    /// offsets and total size as the LLVM value — and carries both the tag byte and a distinct
    /// type identity, enough for a debugger to locate the discriminant and for a pretty-printer
    /// to dispatch. The one place a slot's *encoding* (not its offset/size) is approximate is
    /// the built-in `Result`, whose payload is sized per construction and can be either a
    /// `double` or a pointer (both 8 bytes): the slot is typed `Num`, so a pointer payload reads
    /// back as a float until a `Result`-aware formatter reinterprets it.
    ///
    /// It is deliberately NOT a DWARF variant part (`DW_TAG_variant_part` / `DW_TAG_variant`):
    /// that self-describing form — where each variant names its own fields (`Circle(r)`,
    /// `Rect(w, h)`) and the discriminant maps tag -> variant — is a possible later refinement.
    /// A formatter that renders `Ok(5)`/`Red` without a side-channel tag->variant/payload table
    /// would want a real variant part (built via raw `llvm-sys`, since inkwell 0.10 has no
    /// wrapper — the same path `declare` already uses). Until then the tag + type name are
    /// present, and the choice between variant-part DWARF and a generated tag table is open.
    pub fn sum_type(&self, name: &str, payload_slots: &[DIType<'ctx>]) -> DIType<'ctx> {
        let tag = self.basic_type("i8", 8, DW_ATE_UNSIGNED);
        let mut members: Vec<(&str, DIType<'ctx>)> = vec![("tag", tag)];
        let labels: Vec<String> = (0..payload_slots.len())
            .map(|i| format!("payload{i}"))
            .collect();
        for (label, slot) in labels.iter().zip(payload_slots) {
            members.push((label.as_str(), *slot));
        }
        self.struct_type(name, &members)
    }

    /// Build a named DWARF struct from `members`, laying each out at its natural alignment so
    /// the offsets/sizes match LLVM's default (non-packed) struct layout on x86-64. Shared by
    /// every composite builder above.
    fn struct_type(&self, name: &str, members: &[(&str, DIType<'ctx>)]) -> DIType<'ctx> {
        let scope = self.file.as_debug_info_scope();
        let mut elements = Vec::with_capacity(members.len());
        let mut offset_bits = 0u64;
        let mut struct_align = 8u32;
        for (mname, mty) in members {
            let size = mty.get_size_in_bits();
            let align = member_align_bits(*mty);
            struct_align = struct_align.max(align);
            let member_offset = align_up(offset_bits, align as u64);
            let member = self.builder.create_member_type(
                scope,
                mname,
                self.file,
                0,
                size,
                align,
                member_offset,
                DIFlags::PUBLIC,
                *mty,
            );
            elements.push(member.as_type());
            offset_bits = member_offset + size;
        }
        let size_in_bits = align_up(offset_bits, struct_align as u64);
        self.builder
            .create_struct_type(
                self.compile_unit.as_debug_info_scope(),
                name,
                self.file,
                0,
                size_in_bits,
                struct_align,
                DIFlags::PUBLIC,
                None,
                &elements,
                0,
                None,
                name,
            )
            .as_type()
    }

    // ---- Local variables ------------------------------------------------------------

    /// Create a `DILocalVariable` for a function parameter (`arg_no` is its 1-based index),
    /// scoped to `scope` (the function's subprogram) and typed `ty`.
    pub fn create_parameter(
        &self,
        scope: DIScope<'ctx>,
        name: &str,
        arg_no: u32,
        span: &Span,
        ty: DIType<'ctx>,
    ) -> DILocalVariable<'ctx> {
        let (line, _) = self.line_col(span.start as usize);
        self.builder.create_parameter_variable(
            scope,
            name,
            arg_no,
            self.file,
            line,
            ty,
            true,
            DIFlags::ZERO,
        )
    }

    /// Create a `DILocalVariable` for a `=`/`:=` local, scoped to `scope` and typed `ty`.
    pub fn create_local(
        &self,
        scope: DIScope<'ctx>,
        name: &str,
        span: &Span,
        ty: DIType<'ctx>,
    ) -> DILocalVariable<'ctx> {
        let (line, _) = self.line_col(span.start as usize);
        self.builder
            .create_auto_variable(scope, name, self.file, line, ty, true, DIFlags::ZERO, 0)
    }

    /// Attach `var` to its storage `slot` with a `#dbg_declare` record at the end of `block`,
    /// located at `loc`. The debugger then knows the variable lives at `slot` for the whole
    /// enclosing scope.
    ///
    /// This calls the LLVM-C `InsertDeclareRecordAtEnd` entry point directly rather than
    /// inkwell's `insert_declare_at_end`: under LLVM's new debug-records format (the default
    /// from LLVM 19 on) that wrapper mis-casts the returned `DbgRecord` to an `InstructionValue`
    /// and panics on an `is_instruction()` assertion. Emitting the record ourselves sidesteps
    /// the broken conversion; we don't need the returned handle.
    pub fn declare(
        &self,
        slot: PointerValue<'ctx>,
        var: DILocalVariable<'ctx>,
        loc: DILocation<'ctx>,
        block: BasicBlock<'ctx>,
    ) {
        use inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDeclareRecordAtEnd;
        use inkwell::values::AsValueRef;

        // An empty DWARF expression: the variable's value IS the contents at `slot`.
        let expr = self.builder.create_expression(vec![]);
        unsafe {
            LLVMDIBuilderInsertDeclareRecordAtEnd(
                self.builder.as_mut_ptr(),
                slot.as_value_ref(),
                var.as_mut_ptr(),
                expr.as_mut_ptr(),
                loc.as_mut_ptr(),
                block.as_mut_ptr(),
            );
        }
    }

    /// A nested lexical block scope under `parent`, beginning at `span` — the scope for
    /// variables introduced inside a `{ }` block, so a debugger nests them correctly.
    pub fn lexical_block(&self, parent: DIScope<'ctx>, span: &Span) -> DIScope<'ctx> {
        let (line, col) = self.line_col(span.start as usize);
        self.builder
            .create_lexical_block(parent, self.file, line, col)
            .as_debug_info_scope()
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
