//! Debug-info emission: attaching DWARF scopes, locations, and types to what the
//! generator emits. The `DebugInfo` state itself lives in `codegen::debug`.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Turn on DWARF line-number debug-info emission for this module, using `source`
    /// (the text compiled from `file_path`) to map span byte offsets to `(line, column)`.
    /// Only the native `--debug` build path calls this; without it the generator emits no
    /// debug info at all. `imported_item_count` is how many leading top-level items came from
    /// imported modules (their spans can't be mapped to this file, so they get no debug
    /// info). Must be called before [`generate`].
    pub fn enable_debug(
        &mut self,
        file_path: &std::path::Path,
        source: &str,
        imported_item_count: usize,
    ) {
        self.debug = Some(DebugInfo::new(
            &self.module,
            self.context,
            file_path,
            source,
        ));
        self.di_imported_boundary = imported_item_count;
    }

    /// Point the builder's current debug location at `span` within the function currently
    /// being emitted. A no-op unless debug info is on and a function scope is active — so
    /// call sites need no `if debug` guard of their own.
    pub(super) fn set_debug_loc(&self, span: &Span) {
        if self.di_suppressed {
            return;
        }
        if let (Some(debug), Some(scope)) = (self.debug.as_ref(), self.di_scope) {
            let loc = debug.location(self.context, span, scope);
            self.builder.set_current_debug_location(loc);
        }
    }

    /// Begin emitting the body of `function` (named `name`, starting at `span`) under debug
    /// info: create its `DISubprogram`, attach it, and make it the active source scope.
    /// Returns the previously active scope, which the caller restores via [`end_di_scope`]
    /// once the body is emitted — so a nested function (closure/local fn) does not leave the
    /// enclosing function attributed to the wrong subprogram. A no-op (returns `None`) when
    /// debug info is off.
    pub(super) fn begin_di_function(
        &mut self,
        function: FunctionValue<'ctx>,
        name: &str,
        span: &Span,
    ) -> Option<DIScope<'ctx>> {
        let saved = self.di_scope;
        if self.di_suppressed {
            return saved;
        }
        if let Some(debug) = self.debug.as_ref() {
            let subprogram = debug.create_function(name, span);
            function.set_subprogram(subprogram);
            self.di_scope = Some(subprogram.as_debug_info_scope());
            // Seed the body's leading instructions (parameter stores, TCO back-edge) with a
            // location at the function header, before per-expression locations take over.
            self.set_debug_loc(span);
        }
        saved
    }

    /// Restore the source scope saved by [`begin_di_function`] after a function body is done.
    pub(super) fn end_di_scope(&mut self, saved: Option<DIScope<'ctx>>) {
        self.di_scope = saved;
    }

    /// Enter a nested lexical scope for a `{ }` block starting at `span`, so variables it
    /// introduces nest under a `DW_TAG_lexical_block` rather than the function directly.
    /// Returns the scope to restore via [`end_di_scope`]; a no-op (returns the current scope)
    /// when debug info is off/suppressed.
    pub(super) fn begin_di_lexical_block(&mut self, span: &Span) -> Option<DIScope<'ctx>> {
        let saved = self.di_scope;
        if self.di_suppressed {
            return saved;
        }
        if let (Some(debug), Some(parent)) = (self.debug.as_ref(), self.di_scope) {
            self.di_scope = Some(debug.lexical_block(parent, span));
        }
        saved
    }

    /// The DWARF type for the value representation of Quilon type `ty`, under `--debug`.
    /// `None` when debug info is off. Composites are cached by a structural key so each
    /// distinct Quilon type is emitted once and shared by all its variables — which is what
    /// makes `Text`, `[]T`, records and sum types show up as DISTINCT `DW_AT_type`s even
    /// though they share a `{ptr, i64}`-ish LLVM shape.
    pub(super) fn di_type(&self, ty: &Type) -> Option<DIType<'ctx>> {
        let debug = self.debug.as_ref()?;
        // Scalars carry no structure to cache and `create_basic_type` already dedups by
        // (name, size, encoding) — so return them directly, skipping the key allocation and
        // cache/recursion bookkeeping that only the composites below need.
        match ty {
            Type::Num => return Some(debug.num_type()),
            Type::Bool => return Some(debug.bool_type()),
            Type::Unit => return Some(debug.unit_type()),
            Type::Generic { .. } | Type::Function { .. } => return Some(debug.opaque_pointer()),
            // Maps and Sets are opaque runtime pointers with no DWARF-visible structure.
            Type::Map(_, _) | Type::Set(_) => return Some(debug.opaque_pointer()),
            _ => {}
        }
        let key = self.di_type_key(ty);
        if let Some(t) = debug.cached_type(&key) {
            return Some(t);
        }
        // Break a (hypothetical) recursive type: if this key is already being built, hand back
        // an opaque pointer rather than recursing forever.
        if !self.di_building.borrow_mut().insert(key.clone()) {
            return Some(debug.opaque_pointer());
        }
        let built = self.build_di_type(debug, &key, ty);
        self.di_building.borrow_mut().remove(&key);
        debug.cache_type(&key, built);
        Some(built)
    }

    /// Build (uncached) the DWARF type for composite `ty`, whose already-computed structural
    /// `key` doubles as the DWARF name for the unnamed composites (arrays, anonymous records).
    /// Scalars are handled in [`di_type`] and never reach here. See [`di_type`] for the
    /// distinctness contract.
    pub(super) fn build_di_type(
        &self,
        debug: &DebugInfo<'ctx>,
        key: &str,
        ty: &Type,
    ) -> DIType<'ctx> {
        match ty {
            Type::Text => debug.text_type(),
            Type::Array(elem) => {
                let elem_ty = self.di_type(elem).unwrap_or_else(|| debug.num_type());
                debug.array_type(key, elem_ty)
            }
            Type::Record(fields) => {
                let members = self.di_record_members(debug, fields);
                debug.record_type(key, &members)
            }
            // A named type that resolves to a registered sum is a sum; otherwise a record.
            Type::Named { name, .. } if self.resolves_to_sum(name) => {
                self.di_sum_type(debug, name, &[])
            }
            Type::Named { name, fields, .. } => {
                // Prefer the type's own fields; fall back to the registered record definition
                // (borrowed, not cloned — this only runs on a cache miss but stays cheap).
                let from_map;
                let field_defs: &[(String, Type)] = if !fields.is_empty() {
                    fields
                } else {
                    from_map = self.record_field_types.get(name);
                    from_map.map(Vec::as_slice).unwrap_or(&[])
                };
                let members = self.di_record_members(debug, field_defs);
                debug.record_type(name, &members)
            }
            Type::Sum { name, variants } => self.di_sum_type(debug, name, variants),
            // Scalars / opaque types are resolved in `di_type` before reaching here.
            _ => debug.opaque_pointer(),
        }
    }

    /// Whether the type named `name` denotes a sum type (a registered user sum, or the
    /// built-in `Result`) rather than a record. The single source of truth shared by
    /// `build_di_type` and `di_type_key` so their dispatch and cache key never drift.
    pub(super) fn resolves_to_sum(&self, name: &str) -> bool {
        self.sum_layouts.contains_key(name) || name == "Result"
    }

    /// Lower record `fields` (name + type) to DWARF `(name, DIType)` members.
    pub(super) fn di_record_members(
        &self,
        debug: &DebugInfo<'ctx>,
        fields: &[(String, Type)],
    ) -> Vec<(String, DIType<'ctx>)> {
        fields
            .iter()
            .map(|(fname, fty)| {
                let dt = self.di_type(fty).unwrap_or_else(|| debug.num_type());
                (fname.clone(), dt)
            })
            .collect()
    }

    /// Build a sum type's DWARF entry: `{ i8 tag, payload... }`. The payload slots follow the
    /// same canonical layout as `sum_value_struct_type` — one slot per payload position, typed
    /// by the first concrete (non-generic, non-Unit) field a variant carries there. `variants`
    /// may be the type's own list; when empty (e.g. a `Type::Named`/`Result`), the registered
    /// definition is used. The sizes line up with `register_sum_variants`'s LLVM slots — a
    /// bit-less Unit position is an `i8`, an absent/generic one a `Num` — so the DWARF struct
    /// matches the value in memory.
    pub(super) fn di_sum_type(
        &self,
        debug: &DebugInfo<'ctx>,
        name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> DIType<'ctx> {
        // `Result` has ONE canonical `{ptr,i64}` payload slot (any payload is packed into
        // it — see `register_builtin_sum_types`), so its DWARF slot is that struct, not the
        // per-position widest field the branch below derives for user sum types.
        if name == "Result" {
            return debug.sum_type(name, &[debug.ptr_len_slot()]);
        }
        // Borrow the variant list rather than clone it (this only runs on a cache miss).
        let from_defs;
        let variants: &[crate::ast::SumVariant] = if !variants.is_empty() {
            variants
        } else {
            from_defs = self.sum_variant_defs.get(name);
            from_defs.map(Vec::as_slice).unwrap_or(&[])
        };
        let max_fields = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        let mut slots: Vec<DIType<'ctx>> = Vec::with_capacity(max_fields);
        for i in 0..max_fields {
            let concrete = variants
                .iter()
                .filter_map(|v| v.fields.get(i))
                .find(|f| !matches!(f, Type::Generic { .. } | Type::Unit));
            let slot = match concrete {
                Some(f) => self.di_type(f).unwrap_or_else(|| debug.unit_type()),
                None => debug.unit_type(),
            };
            slots.push(slot);
        }
        // A nullary sum (a payload-free enum) still gets one slot so its `{ i8, .. }` shape is
        // uniform — a `Num`-sized (8-byte) slot, matching `register_sum_variants`'s `double`
        // placeholder. (`Result` is handled by the early return above, not here.)
        if slots.is_empty() {
            slots.push(debug.num_type());
        }
        debug.sum_type(name, &slots)
    }

    /// A structural cache key for `ty`'s DWARF type. Named records/sums key by name (their
    /// field/variant set is fixed per name); anonymous records key by their field structure.
    pub(super) fn di_type_key(&self, ty: &Type) -> String {
        match ty {
            Type::Num => "Num".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Unit => "$".to_string(),
            Type::Text => "Text".to_string(),
            Type::Array(elem) => format!("[]{}", self.di_type_key(elem)),
            Type::Map(k, v) => format!("map${}${}", self.di_type_key(k), self.di_type_key(v)),
            Type::Set(elem) => format!("set${}", self.di_type_key(elem)),
            Type::Record(fields) => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(n, t)| format!("{n}:{}", self.di_type_key(t)))
                    .collect();
                format!("rec{{{}}}", inner.join(","))
            }
            // A named type that resolves to a registered sum keys the SAME as a `Type::Sum` of
            // that name, so the one logical type isn't emitted (and cached) twice.
            Type::Named { name, .. } if self.resolves_to_sum(name) => format!("sum${name}"),
            Type::Named { name, .. } => format!("named${name}"),
            Type::Sum { name, .. } => format!("sum${name}"),
            Type::Generic { name, .. } => format!("gen${name}"),
            Type::Function { .. } => "fn".to_string(),
        }
    }

    /// Emit a `DILocalVariable` + `llvm.dbg.declare` for a parameter or `=`/`:=` local named
    /// `name`, stored at `slot`, of Quilon type `qty`, declared at `span`. `arg_no` is the
    /// 1-based parameter index for a parameter, or `None` for a local. A no-op unless debug
    /// info is on, not suppressed, and a function scope is active — so call sites stay guard-free.
    pub(super) fn declare_variable(
        &self,
        name: &str,
        slot: PointerValue<'ctx>,
        qty: &Type,
        span: &Span,
        arg_no: Option<u32>,
    ) {
        if self.di_suppressed {
            return;
        }
        let (Some(debug), Some(scope)) = (self.debug.as_ref(), self.di_scope) else {
            return;
        };
        let Some(block) = self.builder.get_insert_block() else {
            return;
        };
        let Some(dty) = self.di_type(qty) else {
            return;
        };
        let var = match arg_no {
            Some(n) => debug.create_parameter(scope, name, n, span, dty),
            None => debug.create_local(scope, name, span, dty),
        };
        let loc = debug.location(self.context, span, scope);
        debug.declare(slot, var, loc, block);
    }
}
