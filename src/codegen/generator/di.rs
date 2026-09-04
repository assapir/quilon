//! Debug-info emission: attaching DWARF scopes, locations, and types to what the
//! generator emits. The `DebugInfo` state itself lives in `codegen::debug`.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Turn on DWARF line-number debug-info emission for this module. `file_path` is the root
    /// `.qn` source; `sources` carries the text and display path of it and every imported
    /// module, so each gets a `DIFile` + line table and a span maps to whichever file it
    /// belongs to — an imported/corelib function's debug info points at that module's source.
    /// Only the native `--debug` build path calls this; without it the generator emits no
    /// debug info at all. Must be called before [`generate`].
    pub fn enable_debug(
        &mut self,
        file_path: &std::path::Path,
        sources: &crate::source_map::SourceMap,
    ) {
        self.debug = Some(DebugInfo::new(
            &self.module,
            self.context,
            file_path,
            sources,
        ));
    }

    /// Point the builder's current debug location at `span` within the function currently
    /// being emitted. A no-op unless debug info is on and a function scope is active — so
    /// call sites need no `if debug` guard of their own.
    pub(super) fn set_debug_loc(&self, span: &Span) {
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

    /// Like [`begin_di_function`], but for a generated entry shim (the C `main` wrapper or the
    /// `__ql_entry` fiber thunk): its `DISubprogram` is named for the user's `^` entry point and
    /// marked artificial, so a backtrace attributes the entry frame to `^` and treats the shim
    /// as compiler glue rather than showing the internal `main`/thunk symbol as user code.
    pub(super) fn begin_di_entry_shim(
        &mut self,
        function: FunctionValue<'ctx>,
        span: &Span,
    ) -> Option<DIScope<'ctx>> {
        let saved = self.di_scope;
        if let Some(debug) = self.debug.as_ref() {
            let subprogram = debug.create_entry_shim("^", span);
            function.set_subprogram(subprogram);
            self.di_scope = Some(subprogram.as_debug_info_scope());
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
    /// when debug info is off.
    pub(super) fn begin_di_lexical_block(&mut self, span: &Span) -> Option<DIScope<'ctx>> {
        let saved = self.di_scope;
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
            Type::Num => {
                self.enqueue_render_thunk(ty);
                return Some(debug.num_type());
            }
            Type::Bool => {
                self.enqueue_render_thunk(ty);
                return Some(debug.bool_type());
            }
            Type::Unit => {
                self.enqueue_render_thunk(ty);
                return Some(debug.unit_type());
            }
            Type::Generic { .. } | Type::Function { .. } => return Some(debug.opaque_pointer()),
            _ => {}
        }
        let key = self.di_type_key(ty);
        // Every type that reaches here (Text, an array, a record, a sum, a Map/Set — but not
        // a scalar, handled above, or the opaque Function/Generic case, excluded by
        // `enqueue_render_thunk` itself) gets a render thunk queued, whether this call is a
        // cache hit or a fresh build — including a type reached only NESTED (an array
        // element, a record field, a Map/Set's key/value) via this same function's own
        // recursion in `build_di_type`, not just one reaching `declare_variable` directly.
        self.enqueue_render_thunk(ty);
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
                // NOT `key` (`di_type_key`, which prefixes a nested Named/Sum element with
                // `named$`/`sum$` — e.g. `"[]named$Point"`): the render thunk (and the lldb
                // formatter reading a live `DW_AT_name` back) both derive the thunk symbol
                // from `di_debug_name`, so the array's OWN `DW_AT_name` has to be built from
                // that same function or the two would disagree for an array of a user
                // record/sum. `di_debug_name` already matches `key` for every scalar/Text
                // element (its fallback is `di_type_key` itself), so this only changes an
                // array of a NAMED/Sum element's displayed name — for the readable "[]Point"
                // (matching Map/Set's own naming) rather than the cache-key shape.
                debug.array_type(&self.di_debug_name(ty), elem_ty)
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
            // A Map/Set value is a pointer-sized runtime handle, named for its element
            // types (`"Map[Text, Num]"`, `"Set[Num]"`) rather than left an anonymous
            // opaque pointer — that name is what the `--debug` render thunk and the lldb
            // formatter derive the value's `` ` `` rendering from. Deliberately NOT `key`
            // (the cache key, `"map$Text$Num"`): the display name and the cache key are
            // free to differ, and here they must — a debugger reads only `DW_AT_name`.
            Type::Map(_, _) | Type::Set(_) => debug.collection_type(&self.di_debug_name(ty)),
            // Scalars / opaque types are resolved in `di_type` before reaching here.
            _ => debug.opaque_pointer(),
        }
    }

    /// The debug-info DISPLAY name for `ty` — what `DW_AT_name` records: `"Num"`, `"[]Num"`,
    /// `"Point"`, `"Result"`, `"Map[Text, Num]"`, `"Set[Num]"`. Distinct from [`di_type_key`]
    /// (a cache key, free to use a different format) only for `Map`/`Set`, which get a
    /// readable `Ctor[Args]` name instead of the key's `map$K$V` shape — every other case
    /// already uses the display form as its key, so this simply mirrors that. Used both to
    /// build a composite's own `DW_AT_name` ([`build_di_type`]'s `Map`/`Set` arm) and,
    /// recursively, to build ITS nested elements' names — a `Num` inside a `Map[Text, Num]`
    /// contributes the literal substring `"Num"` here, regardless of what a live debugger
    /// happens to display for a STANDALONE `Num` variable (see
    /// [`render_thunk_debug_name`] for that divergent case): a composite's `DW_AT_name` is
    /// an opaque string a debugger never reinterprets, so nesting stays purely textual.
    pub(super) fn di_debug_name(&self, ty: &Type) -> String {
        match ty {
            Type::Map(k, v) => format!("Map[{}, {}]", self.di_debug_name(k), self.di_debug_name(v)),
            Type::Set(elem) => format!("Set[{}]", self.di_debug_name(elem)),
            Type::Array(elem) => format!("[]{}", self.di_debug_name(elem)),
            Type::Named { name, .. } | Type::Sum { name, .. } => name.clone(),
            _ => self.di_type_key(ty),
        }
    }

    /// What a debugger ACTUALLY reads back as `ty`'s type name — the string
    /// [`render_thunk_symbol`] derives a thunk's symbol from on BOTH sides (this Rust-side
    /// emission, and `editors/vscode/formatters/quilon.py`'s `sanitize_debug_type_name`
    /// reading a live value's `SBType.GetName()`). [`di_debug_name`] for every composite
    /// (`Text`, an array, a record, a sum, a Map/Set): a debugger shows a
    /// `DW_TAG_structure_type`'s own name faithfully, confirmed against a real lldb session.
    ///
    /// For a bare SCALAR (`Num`/`Bool`/`Unit`) this instead returns lldb's OWN canonicalized
    /// name, which does NOT match the `DW_AT_name` `debug.rs`'s `num_type`/`bool_type`/
    /// `unit_type` give the DWARF entry (`"Num"`/`"Bool"`/`"$"`): lldb's DWARF importer
    /// derives a `DW_TAG_base_type`'s displayed name from its `(encoding, size)` pair alone,
    /// ignoring whatever name the DWARF gives it — confirmed live, an `f64`/`DW_ATE_float`
    /// reads back as `"double"`, an `i1`/`DW_ATE_boolean` as `"bool"`, an `i8`/
    /// `DW_ATE_unsigned` as `"unsigned char"`. This divergence applies ONLY at the top
    /// level: a scalar NESTED inside a composite's own name (`di_debug_name`'s `Map`/`Set`/
    /// `Array` recursion) still contributes its ordinary Quilon name — that name is an
    /// opaque substring of the composite's `DW_AT_name`, which lldb never reinterprets.
    pub(super) fn render_thunk_debug_name(&self, ty: &Type) -> String {
        match ty {
            Type::Num => "double".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Unit => "unsigned char".to_string(),
            _ => self.di_debug_name(ty),
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
                // A named-RECORD payload rides in the slot by pointer (the record ABI —
                // see `register_sum_variants`/`type_to_llvm`), so its DWARF slot is a
                // pointer, not the record struct laid out by value.
                Some(Type::Named { name, .. }) if !self.resolves_to_sum(name) => {
                    debug.opaque_pointer()
                }
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
    /// info is on and a function scope is active — so call sites stay guard-free. `di_type`
    /// (called below) queues `qty`'s `--debug` render thunk as a side effect; see
    /// [`enqueue_render_thunk`]/[`drain_pending_render_thunks`].
    pub(super) fn declare_variable(
        &self,
        name: &str,
        slot: PointerValue<'ctx>,
        qty: &Type,
        span: &Span,
        arg_no: Option<u32>,
    ) {
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

    /// Queue `ty` for a `--debug` render thunk, at most once per structural key (see
    /// [`di_type_key`]). Called from `di_type` itself — cheap and `&self` (a `RefCell`
    /// dedupe set + a pending-list push), so it runs equally for a type reaching
    /// [`declare_variable`] directly and one reached only NESTED, through `di_type`'s own
    /// recursion while building a composite (an array element, a record field, a Map/Set's
    /// key/value). [`drain_pending_render_thunks`] later emits the actual IR, which needs
    /// `&mut self`. Skips `Function`/`Generic`: neither has a concrete static rendering (a
    /// function value isn't renderable at all; a surviving `Generic` payload has no fixed
    /// type), matching `di_type`'s own early opaque-pointer return for them.
    fn enqueue_render_thunk(&self, ty: &Type) {
        if matches!(ty, Type::Function { .. } | Type::Generic { .. }) {
            return;
        }
        let key = self.di_type_key(ty);
        if self.di_render_thunks.borrow_mut().insert(key) {
            self.di_pending_thunks.borrow_mut().push(ty.clone());
        }
    }

    /// Emit every render thunk [`enqueue_render_thunk`] queued while generating the program
    /// — called once, near the end of [`super::CodeGenerator::generate`], after every
    /// function body (so every `di_type` call that will ever run already has). A no-op when
    /// debug info is off: nothing is ever queued then. Emission failures are logged and
    /// swallowed — a `--debug` build should not fail over a debugger convenience.
    pub(super) fn drain_pending_render_thunks(&mut self) {
        loop {
            // Popped into an owned value in its own statement, so the `RefCell` borrow ends
            // before `emit_render_thunk` (which needs `&mut self`) runs.
            let next = self.di_pending_thunks.borrow_mut().pop();
            let Some(ty) = next else { break };
            if let Err(e) = self.emit_render_thunk(&ty) {
                let key = self.di_type_key(&ty);
                eprintln!("warning: quilon --debug: failed to emit render thunk for {key}: {e}");
            }
        }
    }

    /// Emit the thunk `drain_pending_render_thunks` gates: `const char*
    /// __qn_render$<name>(const void* slot)`. Loads the value at `slot` (the variable's own
    /// storage — the alloca `llvm.dbg.declare` points at) with `ty`'s value representation,
    /// renders it through the SAME path `io.print`/interpolation use ([`render_value`]), and
    /// returns a NUL-terminated GC copy of the rendered bytes via the `__render_c_string`
    /// runtime helper (a debugger evaluates a C-ABI call and expects a C string back, not the
    /// `{ptr, i64}` ABI a Quilon caller uses). Exported (external linkage) under its
    /// `render_thunk_symbol` name so `nm`/a debugger can find it by symbol.
    fn emit_render_thunk(&mut self, ty: &Type) -> Result<(), String> {
        let symbol = crate::codegen::debug::render_thunk_symbol(&self.render_thunk_debug_name(ty));
        if self.module.get_function(&symbol).is_some() {
            return Ok(());
        }
        let ptr = self.context.ptr_type(AddressSpace::default());
        let value_llvm = self.value_repr_type(ty)?;

        // Emitting a fresh function mid-stream: save and restore the enclosing builder
        // position, current function, and debug location so the surrounding emission
        // resumes intact (mirrors `emit_key_trampolines` in `collections.rs`).
        let saved_block = self.builder.get_insert_block();
        let saved_function = self.current_function;
        let saved_loc = self.builder.get_current_debug_location();
        self.builder.unset_current_debug_location();

        let thunk = self
            .module
            .add_function(&symbol, ptr.fn_type(&[ptr.into()], false), None);
        thunk.set_linkage(inkwell::module::Linkage::External);
        self.current_function = Some(thunk);
        let entry = self.context.append_basic_block(thunk, "entry");
        self.builder.position_at_end(entry);
        let slot = thunk.get_nth_param(0).unwrap().into_pointer_value();
        let value = self
            .builder
            .build_load(value_llvm, slot, "render_slot")
            .map_err(ctx("Failed to load render-thunk slot"))?;
        let text = self.render_value(ty, value)?;
        let (data, len) = self.split_text(text.into_struct_value(), "render_thunk")?;
        let c_string_fn = self.get_intrinsic("__render_c_string")?;
        let call = self
            .builder
            .build_call(c_string_fn, &[data.into(), len.into()], "render_c_str")
            .map_err(ctx("Failed to call __render_c_string"))?;
        let out_ptr = Self::call_result_to_basic(call)?.into_pointer_value();
        self.builder
            .build_return(Some(&out_ptr))
            .map_err(ctx("Failed to return from render thunk"))?;

        self.current_function = saved_function;
        if let Some(block) = saved_block {
            self.builder.position_at_end(block);
        }
        if let Some(loc) = saved_loc {
            self.builder.set_current_debug_location(loc);
        }
        Ok(())
    }
}
