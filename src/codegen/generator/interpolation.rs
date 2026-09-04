//! String interpolation: lowering an interpolated literal to the `Text` it renders,
//! one piece at a time.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower an `Expression::Interpolation`: render each hole to `Text` through its `` ` ``
    /// operator and concatenate the literal chunks and rendered holes left to right.
    pub(super) fn generate_interpolation(
        &mut self,
        parts: &[InterpolationPart],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let mut acc: Option<BasicValueEnum<'ctx>> = None;
        for part in parts {
            let piece = match part {
                InterpolationPart::Literal(s) => self.text_literal(s)?,
                InterpolationPart::Hole(e) => self.render_expression(e)?,
            };
            acc = Some(match acc {
                None => piece,
                Some(a) => {
                    self.generate_text_concat(a.into_struct_value(), piece.into_struct_value())?
                }
            });
        }
        match acc {
            Some(v) => Ok(v),
            None => self.text_literal(""),
        }
    }

    /// Render `expression` to a `Text` value: evaluate it, then dispatch on its authoritative
    /// Quilon type (via the oracle) to the right renderer. The single render path shared
    /// by string interpolation and `print`/`eprint`.
    pub(super) fn render_expression(
        &mut self,
        expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ty = self.infer_type(expression);
        let value = self.generate_expression(expression)?;
        // Break unbounded self-recursion: rendering the receiver `it` WHOLESALE inside its
        // own type's `` ` `` override would invoke that override forever. That one case
        // renders via the built-in default (a record's type name, a sum's variant name); a
        // DIFFERENT value of the same type — e.g. a child node `it.next` — still uses the
        // override and terminates for any finite structure.
        if let Expression::Identifier { name, .. } = expression
            && name == crate::ast::RECEIVER
            && let Type::Named { name: ty_name, .. } | Type::Sum { name: ty_name, .. } = &ty
            && self.generating_backtick_for.as_deref() == Some(ty_name.as_str())
        {
            return self.render_builtin_default(&ty, value);
        }
        self.render_value(&ty, value)
    }

    /// Render `value` (of Quilon type `ty`) to a `Text` `{ptr,i64}` value. A type with its
    /// own `` ` `` override renders through that method; every other type uses the built-in
    /// default (see the rendering table in docs/types/text.md). Dispatch is by the authoritative
    /// oracle type `ty` — source positions carry file identity, so the oracle
    /// is reliable across imported modules and needs no shape-based hedge.
    pub(super) fn render_value(
        &mut self,
        ty: &Type,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match ty {
            // A number: integer-valued without decimals, else shortest round-trip. A
            // not-yet-concrete sum payload (`Generic`) is represented as a Num.
            Type::Num | Type::Generic { .. } => {
                self.render_scalar_intrinsic("__num_to_text", value.into())
            }
            // A bool renders as `True`/`False` (capitalized, unlike the literals).
            Type::Bool => {
                let b64 = self
                    .builder
                    .build_int_z_extend(value.into_int_value(), self.context.i64_type(), "bool_ext")
                    .map_err(ctx("Failed to extend bool"))?;
                self.render_scalar_intrinsic("__bool_to_text", b64.into())
            }
            // A `Text` renders as itself.
            Type::Text => Ok(value),
            Type::Unit => self.text_literal("$"),
            // A record: its own `` ` `` override (called with the record pointer), else the
            // type name.
            Type::Named { name, .. } => {
                if self.render_overrides.contains(name) {
                    self.call_render_override(name, value)
                } else {
                    self.text_literal(name)
                }
            }
            // An anonymous record has no name to show.
            Type::Record(_) => self.text_literal("record"),
            // A sum value: its own `` ` `` override (called with the sum value), else the
            // variant/constructor name.
            Type::Sum { name, .. } => {
                if self.render_overrides.contains(name) {
                    self.call_render_override(name, value)
                } else {
                    self.render_sum_variant(name, value)
                }
            }
            // An array renders its elements (truncated past 10, see `render_array`).
            Type::Array(elem) => self.render_array(elem, value),
            // A Map renders its entries `[|k => v, ...|]`; a Set its elements `[|e, ...|]` —
            // each key/value/element through its own `` ` ``. Iteration order is unspecified
            // (see docs/collections/), so which entry prints first is not guaranteed.
            Type::Map(key, val) => self.render_map(key, val, value),
            Type::Set(elem) => self.render_set(elem, value),
            Type::Function { .. } => self.text_literal("<function>"),
        }
    }

    /// Render `value` with the type's BUILT-IN default `` ` ``, bypassing any user override —
    /// used to terminate an override that renders its own receiver `it` wholesale. A record
    /// renders as its type name, a sum as its variant name; any other type has only its one
    /// built-in rendering, so it takes the normal path.
    fn render_builtin_default(
        &mut self,
        ty: &Type,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match ty {
            Type::Named { name, .. } => {
                let name = name.clone();
                self.text_literal(&name)
            }
            Type::Sum { name, .. } => {
                let name = name.clone();
                self.render_sum_variant(&name, value)
            }
            _ => self.render_value(ty, value),
        }
    }

    /// Invoke a user type's `` ` `` render override (the `Type_op$backtick` method) — the
    /// receiver crosses as a record pointer or a sum value, matching the method's emission.
    pub(super) fn call_render_override(
        &mut self,
        name: &str,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let sym = method_symbol(name, "`");
        let f = self
            .module
            .get_function(&sym)
            .ok_or_else(|| format!("render override not declared: {}", sym))?;
        let call = self
            .builder
            .build_call(f, &[value.into()], "render")
            .map_err(ctx("Failed to call render override"))?;
        Self::call_result_to_basic(call)
    }

    /// Call a `f64|i64 -> {ptr,i64}` render intrinsic (`__num_to_text` / `__bool_to_text`).
    pub(super) fn render_scalar_intrinsic(
        &mut self,
        name: &str,
        arg: inkwell::values::BasicMetadataValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let f = self.get_intrinsic(name)?;
        let call = self
            .builder
            .build_call(f, &[arg], "render")
            .map_err(|e| format!("Failed to call {}: {:?}", name, e))?;
        Self::call_result_to_basic(call)
    }

    /// Build a `Text` `{ptr,i64}` value for the compile-time-constant string `s` (mirrors
    /// `Expression::String` lowering): a global byte constant plus its byte length.
    pub(super) fn text_literal(&mut self, s: &str) -> Result<BasicValueEnum<'ctx>, String> {
        let global = self
            .builder
            .build_global_string_ptr(s, "rstr")
            .map_err(ctx("Failed to build render literal"))?;
        let len = self.context.i64_type().const_int(s.len() as u64, false);
        let text_ty = self.ptr_len_struct_type();
        let with_ptr = self
            .builder
            .build_insert_value(
                text_ty.get_undef(),
                global.as_pointer_value(),
                0,
                "rtext_ptr",
            )
            .map_err(ctx("Failed to insert render ptr"))?
            .into_struct_value();
        let text = self
            .builder
            .build_insert_value(with_ptr, len, 1, "rtext_len")
            .map_err(ctx("Failed to insert render len"))?
            .into_struct_value();
        Ok(text.into())
    }

    /// Load `slot` (a `Text`), concatenate `piece` onto it, and store the result back.
    pub(super) fn append_text(
        &mut self,
        slot: PointerValue<'ctx>,
        piece: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        let text_ty = self.ptr_len_struct_type();
        let cur = self
            .builder
            .build_load(text_ty, slot, "acc")
            .map_err(ctx("Failed to load render acc"))?
            .into_struct_value();
        let next = self.generate_text_concat(cur, piece.into_struct_value())?;
        self.builder
            .build_store(slot, next)
            .map_err(ctx("Failed to store render acc"))?;
        Ok(())
    }

    /// Render a sum value as its variant/constructor name (the built-in default): extract
    /// the discriminant and `switch` to the matching name string.
    pub(super) fn render_sum_variant(
        &mut self,
        type_name: &str,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let func = self
            .current_function
            .ok_or_else(|| "sum render outside a function".to_string())?;
        let sv = value.into_struct_value();
        let tag = self
            .builder
            .build_extract_value(sv, 0, "sum_tag")
            .map_err(ctx("Failed to extract sum tag"))?
            .into_int_value();

        // All (tag, variant name) pairs of this sum type, in tag order.
        let mut variants: Vec<(u8, String)> = self
            .sum_variants
            .iter()
            .filter(|(_, (_, tn))| tn == type_name)
            .map(|(vname, (t, _))| (*t, vname.clone()))
            .collect();
        variants.sort_by_key(|(t, _)| *t);

        let text_ty = self.ptr_len_struct_type();
        let name_slot = self.create_entry_block_alloca("sum_name", text_ty.into())?;
        let i8t = self.context.i8_type();
        let default_bb = self.context.append_basic_block(func, "sum_default");
        let merge_bb = self.context.append_basic_block(func, "sum_merge");

        let mut case_blocks = Vec::with_capacity(variants.len());
        for (t, vname) in &variants {
            let bb = self.context.append_basic_block(func, "sum_case");
            case_blocks.push((*t, vname.clone(), bb));
        }
        let cases: Vec<(
            inkwell::values::IntValue<'ctx>,
            inkwell::basic_block::BasicBlock<'ctx>,
        )> = case_blocks
            .iter()
            .map(|(t, _, bb)| (i8t.const_int(*t as u64, false), *bb))
            .collect();
        self.builder
            .build_switch(tag, default_bb, &cases)
            .map_err(ctx("Failed to build sum switch"))?;

        for (_, vname, bb) in &case_blocks {
            self.builder.position_at_end(*bb);
            // A qualified variant (`core.http.Get`) renders as the name the user writes
            // and matches on: its last segment.
            let lit = self.text_literal(crate::ast::display_name(vname))?;
            self.builder
                .build_store(name_slot, lit)
                .map_err(ctx("Failed to store variant name"))?;
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(ctx("Failed to branch"))?;
        }
        self.builder.position_at_end(default_bb);
        let unknown = self.text_literal("?")?;
        self.builder
            .build_store(name_slot, unknown)
            .map_err(ctx("Failed to store default name"))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to branch"))?;

        self.builder.position_at_end(merge_bb);
        self.builder
            .build_load(text_ty, name_slot, "sum_name_val")
            .map_err(ctx("Failed to load variant name"))
    }

    /// Render an array: `[a, b, c]` (each element via its own `` ` ``) when the length is
    /// `<= 10`, else a truncated `[first <- last]`. Emits a runtime loop over the elements.
    pub(super) fn render_array(
        &mut self,
        elem_ty: &Type,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let func = self
            .current_function
            .ok_or_else(|| "array render outside a function".to_string())?;
        let i64t = self.context.i64_type();
        let text_ty = self.ptr_len_struct_type();

        let sv = value.into_struct_value();
        let data_ptr = self
            .builder
            .build_extract_value(sv, 0, "arr_data")
            .map_err(ctx("Failed to extract array data"))?
            .into_pointer_value();
        let size = self
            .builder
            .build_extract_value(sv, 1, "arr_size")
            .map_err(ctx("Failed to extract array size"))?
            .into_int_value();
        let elem_llvm = self.value_repr_type(elem_ty)?;

        // Accumulator `Text`, seeded with the opening bracket.
        let acc_slot = self.create_entry_block_alloca("render_acc", text_ty.into())?;
        let open = self.text_literal("[")?;
        self.builder
            .build_store(acc_slot, open)
            .map_err(ctx("Failed to seed render acc"))?;

        let ten = i64t.const_int(10, false);
        let is_small = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLE, size, ten, "arr_small")
            .map_err(ctx("Failed to compare array size"))?;
        let small_bb = self.context.append_basic_block(func, "arr_small");
        let trunc_bb = self.context.append_basic_block(func, "arr_trunc");
        let merge_bb = self.context.append_basic_block(func, "arr_render_merge");
        self.builder
            .build_conditional_branch(is_small, small_bb, trunc_bb)
            .map_err(ctx("Failed to branch on array size"))?;

        // --- Full form: loop `i` in 0..size, comma-separated. ---
        self.builder.position_at_end(small_bb);
        let i_slot = self.create_entry_block_alloca("render_i", i64t.into())?;
        self.builder
            .build_store(i_slot, i64t.const_zero())
            .map_err(ctx("Failed to init loop index"))?;
        let head = self.context.append_basic_block(func, "arr_head");
        let sep_bb = self.context.append_basic_block(func, "arr_sep");
        let elem_bb = self.context.append_basic_block(func, "arr_elem");
        let done = self.context.append_basic_block(func, "arr_done");
        self.builder
            .build_unconditional_branch(head)
            .map_err(ctx("Failed to enter loop"))?;

        self.builder.position_at_end(head);
        let i_cur = self
            .builder
            .build_load(i64t, i_slot, "i")
            .map_err(ctx("Failed to load i"))?
            .into_int_value();
        let cont = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i_cur, size, "i_lt")
            .map_err(ctx("Failed to test loop"))?;
        // Non-zero index appends a separator before the element; index 0 skips it.
        let is_first = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                i_cur,
                i64t.const_zero(),
                "i_first",
            )
            .map_err(ctx("Failed to test first"))?;
        self.builder
            .build_conditional_branch(cont, elem_bb, done)
            .map_err(ctx("Failed to branch loop"))?;
        // `elem_bb` is entered from `head`; from there, choose whether to insert a comma.
        self.builder.position_at_end(elem_bb);
        let after_sep = self.context.append_basic_block(func, "arr_after_sep");
        self.builder
            .build_conditional_branch(is_first, after_sep, sep_bb)
            .map_err(ctx("Failed to branch sep"))?;
        self.builder.position_at_end(sep_bb);
        let comma = self.text_literal(", ")?;
        self.append_text(acc_slot, comma)?;
        self.builder
            .build_unconditional_branch(after_sep)
            .map_err(ctx("Failed to branch after sep"))?;
        self.builder.position_at_end(after_sep);
        let elem = self.load_element(data_ptr, elem_llvm, i_cur)?;
        let etext = self.render_value(elem_ty, elem)?;
        self.append_text(acc_slot, etext)?;
        let inc = self
            .builder
            .build_int_add(i_cur, i64t.const_int(1, false), "i_inc")
            .map_err(ctx("Failed to inc i"))?;
        self.builder
            .build_store(i_slot, inc)
            .map_err(ctx("Failed to store i"))?;
        self.builder
            .build_unconditional_branch(head)
            .map_err(ctx("Failed to loop back"))?;

        self.builder.position_at_end(done);
        let close = self.text_literal("]")?;
        self.append_text(acc_slot, close)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to finish small"))?;

        // --- Truncated form: `[first <- last]`. ---
        self.builder.position_at_end(trunc_bb);
        let first = self.load_element(data_ptr, elem_llvm, i64t.const_zero())?;
        let ftext = self.render_value(elem_ty, first)?;
        self.append_text(acc_slot, ftext)?;
        let arrow = self.text_literal(" <- ")?;
        self.append_text(acc_slot, arrow)?;
        let last_idx = self
            .builder
            .build_int_sub(size, i64t.const_int(1, false), "last_idx")
            .map_err(ctx("Failed to compute last index"))?;
        let last = self.load_element(data_ptr, elem_llvm, last_idx)?;
        let ltext = self.render_value(elem_ty, last)?;
        self.append_text(acc_slot, ltext)?;
        let close2 = self.text_literal("]")?;
        self.append_text(acc_slot, close2)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to finish trunc"))?;

        self.builder.position_at_end(merge_bb);
        self.builder
            .build_load(text_ty, acc_slot, "arr_text")
            .map_err(ctx("Failed to load array text"))
    }
}
