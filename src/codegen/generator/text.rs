//! `Text`: its built-in methods, concatenation, and comparison, over the
//! `{ data, byte_len }` representation.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower a built-in `Text` method call (`args[0]` is the `Text` receiver).
    ///
    /// The PRIMITIVE methods (segmentation, search, slice, the whitespace walks, case
    /// mapping) lower to their `quilon-rt` intrinsics; the COMPOSABLE ones (those
    /// [`crate::ast::qn_text_impl`] names) lower to a plain call of their `core.text`
    /// implementation, with the receiver as the first argument — the module loader merged
    /// those functions in exactly because this call appears. The plain-call machinery also
    /// fills in the trailing `Site` the fail-loud implementations (`repeat`, `replace`,
    /// `replaceAll`) declare, so a violated contract reports where the METHOD call is
    /// written.
    pub(super) fn generate_text_method(
        &mut self,
        method: &str,
        args: &[Expression],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;

        // A composable method: re-enter the call generator as the plain call it desugars
        // to. `args` already leads with the receiver, which is the implementation's first
        // parameter.
        if let Some(implementation) = crate::ast::qn_text_impl(method) {
            let callee = Expression::Identifier {
                name: implementation.to_string(),
                span: span.clone(),
            };
            return self.generate_call(&callee, args, false, span);
        }

        let (recv_ptr, recv_len) = self.extract_text(&args[0])?;

        // Call a struct-returning ({ptr,i64}) Text intrinsic with the given metadata args.
        let call_struct = |this: &mut Self,
                           intr: &str,
                           call_args: &[inkwell::values::BasicMetadataValueEnum<'ctx>]|
         -> Result<BasicValueEnum<'ctx>, String> {
            let f = this.get_intrinsic(intr)?;
            Ok(this
                .builder
                .build_call(f, call_args, "txt_m")
                .map_err(ctx("Failed to call {intr}"))?
                .as_any_value_enum()
                .into_struct_value()
                .into())
        };

        match method {
            "trimStart" => call_struct(
                self,
                "__text_trim_start",
                &[recv_ptr.into(), recv_len.into()],
            ),
            "trimEnd" => call_struct(self, "__text_trim_end", &[recv_ptr.into(), recv_len.into()]),
            "toUpper" => call_struct(self, "__text_to_upper", &[recv_ptr.into(), recv_len.into()]),
            "toLower" => call_struct(self, "__text_to_lower", &[recv_ptr.into(), recv_len.into()]),
            "graphemes" => call_struct(
                self,
                "__text_graphemes",
                &[recv_ptr.into(), recv_len.into()],
            ),
            "at" => {
                // The grapheme at `index`, as an `Ok(Text)`/`NotOk` `Result` — a grapheme
                // is never empty, so the intrinsic's empty answer IS "out of bounds".
                let index = self.text_index_arg(&args[1], "at_idx")?;
                let grapheme = call_struct(
                    self,
                    "__text_at",
                    &[recv_ptr.into(), recv_len.into(), index.into()],
                )?
                .into_struct_value();
                let (_, len) = self.split_text(grapheme, "at")?;
                let found = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::SGT,
                        len,
                        len.get_type().const_zero(),
                        "at_found",
                    )
                    .map_err(ctx("Failed to compare grapheme len"))?;
                self.build_conditional_result(
                    found,
                    self.ptr_len_struct_type().into(),
                    "text_at",
                    |_| Ok(grapheme.into()),
                )
            }
            "slice" => {
                let start = self.text_index_arg(&args[1], "slice_start")?;
                let end = self.text_index_arg(&args[2], "slice_end")?;
                call_struct(
                    self,
                    "__text_slice",
                    &[recv_ptr.into(), recv_len.into(), start.into(), end.into()],
                )
            }
            "indexOf" => self.generate_text_index_of(recv_ptr, recv_len, &args[1]),
            other => Err(format!("unknown text method `{other}`")),
        }
    }

    /// Evaluate a `Text` expression and split it into its `(data_ptr, byte_len)` fields —
    /// the shared primitive for lowering Text-method calls, whose intrinsics take a `Text`
    /// as its two fields.
    pub(super) fn extract_text(
        &mut self,
        expression: &Expression,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        let value = self.generate_expression(expression)?;
        self.text_fields(value)
    }

    /// Split an already-evaluated `Text` into its `(data_ptr, byte_len)` fields — the one
    /// place a `Text`'s `{ptr, i64}` struct is taken apart.
    pub(super) fn text_fields(
        &self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        let BasicValueEnum::StructValue(text) = value else {
            return Err("expected a Text value".to_string());
        };
        self.split_text(text, "txt")
    }

    /// Split an already-evaluated `Text` value into its `(data_ptr, byte_len)` fields, named
    /// after `label`. Every intrinsic that takes a `Text` takes it as this pair — the length
    /// is what bounds the bytes, so no caller has to look for a terminator.
    pub(super) fn split_text(
        &self,
        text: inkwell::values::StructValue<'ctx>,
        label: &str,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        let ptr = self
            .builder
            .build_extract_value(text, 0, &format!("{label}_ptr"))
            .map_err(ctx("Failed to extract text ptr"))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(text, 1, &format!("{label}_len"))
            .map_err(ctx("Failed to extract text len"))?
            .into_int_value();
        Ok((ptr, len))
    }

    /// Evaluate a `Num` index argument (an `f64`) and convert it to the `i64` the Text
    /// intrinsics take (used by `slice`'s start/end).
    /// Narrow an intrinsic's `i64` 0/1 answer to an `i1` Quilon `Bool`. Several runtime
    /// intrinsics return a plain integer because the C ABI has no bool of our width.
    pub(super) fn int_to_bool(
        &self,
        value: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        Ok(self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                value,
                value.get_type().const_zero(),
                name,
            )
            .map_err(ctx("Failed to narrow an intrinsic result to Bool"))?
            .into())
    }

    pub(super) fn text_index_arg(
        &mut self,
        expression: &Expression,
        name: &str,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let f = self.generate_expression(expression)?.into_float_value();
        self.builder
            .build_float_to_signed_int(f, self.context.i64_type(), name)
            .map_err(ctx("Failed to convert text index"))
    }

    /// Lower `Text.indexOf(sub)`: call `__text_index_of` (grapheme index or -1) and turn
    /// the result into a `Result` — `Ok(Num idx)` when >= 0, else `NotOk` — using the
    /// same `{ i8 tag, f64 }` shape `array_at`/`array_find` produce (no -1 sentinel).
    pub(super) fn generate_text_index_of(
        &mut self,
        recv_ptr: PointerValue<'ctx>,
        recv_len: inkwell::values::IntValue<'ctx>,
        sub: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        let (sp, sl) = self.extract_text(sub)?;
        let f = self.get_intrinsic("__text_index_of")?;
        let idx = self
            .builder
            .build_call(
                f,
                &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                "txt_index_of",
            )
            .map_err(ctx("Failed to call __text_index_of"))?
            .as_any_value_enum()
            .into_int_value();

        let i64t = self.context.i64_type();
        let found = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::SGE,
                idx,
                i64t.const_zero(),
                "idx_found",
            )
            .map_err(ctx("Failed to compare index"))?;

        // No branch needed: the Ok payload (`idx` widened to f64) is safe to compute
        // unconditionally, so build both Results eagerly and `select` on `found`.
        let elem_llvm: BasicTypeEnum = self.context.f64_type().into();
        let idx_f = self
            .builder
            .build_signed_int_to_float(idx, self.context.f64_type(), "idx_as_num")
            .map_err(ctx("Failed to convert index to num"))?;
        let ok = self.build_result(elem_llvm, "Ok", idx_f.into())?;
        let no = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm))?;
        self.builder
            .build_select(found, ok, no, "idx_value")
            .map_err(ctx("Failed to select indexOf result"))
    }

    /// Concatenate two `Text` values into a fresh, GC-allocated buffer and return a new
    /// `{ ptr, byte_len }` struct. The buffer holds exactly the concatenated bytes — a
    /// `Text` carries its own length, so nothing reads past it.
    pub(super) fn generate_text_concat(
        &mut self,
        left: inkwell::values::StructValue<'ctx>,
        right: inkwell::values::StructValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i8t = self.context.i8_type();

        let (l_ptr, l_len) = self.split_text(left, "l")?;
        let (r_ptr, r_len) = self.split_text(right, "r")?;

        let total = self
            .builder
            .build_int_add(l_len, r_len, "concat_len")
            .map_err(ctx("Failed to add lengths"))?;

        use inkwell::values::AnyValue;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let dest = self
            .builder
            .build_call(alloc_fn, &[total.into()], "concat_buf")
            .map_err(ctx("Failed to call __alloc"))?
            .as_any_value_enum()
            .into_pointer_value();

        let memcpy_fn = self.get_intrinsic("memcpy")?;
        self.builder
            .build_call(memcpy_fn, &[dest.into(), l_ptr.into(), l_len.into()], "")
            .map_err(ctx("Failed to copy left text"))?;
        let tail = unsafe {
            self.builder
                .build_gep(i8t, dest, &[l_len], "concat_tail")
                .map_err(ctx("Failed to offset into buffer"))?
        };
        self.builder
            .build_call(memcpy_fn, &[tail.into(), r_ptr.into(), r_len.into()], "")
            .map_err(ctx("Failed to copy right text"))?;

        let text_ty = self.ptr_len_struct_type();
        let with_ptr = self
            .builder
            .build_insert_value(text_ty.get_undef(), dest, 0, "cat_ptr")
            .map_err(ctx("Failed to insert concat ptr"))?
            .into_struct_value();
        let text = self
            .builder
            .build_insert_value(with_ptr, total, 1, "cat_len")
            .map_err(ctx("Failed to insert concat len"))?
            .into_struct_value();
        Ok(text.into())
    }

    /// Lower a `Text`-vs-`Text` comparison: call `__text_cmp(aptr, alen, bptr, blen)`
    /// (returns -1/0/1, memcmp-style with the shorter string ordering first on a common
    /// prefix), then compare that i32 result against 0 with the predicate matching `operator`.
    /// Backs `Text` equality and lexicographic ordering (`==`/`!=`/`<`/`<=`/`>`/`>=`).
    pub(super) fn generate_text_compare(
        &mut self,
        operator: BinaryOperator,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) = (lhs, rhs) else {
            return Err("Text comparison requires two Text values".to_string());
        };
        let (l_ptr, l_len) = self.split_text(l, "lcmp")?;
        let (r_ptr, r_len) = self.split_text(r, "rcmp")?;

        let cmp_fn = self.get_intrinsic("__text_cmp")?;
        use inkwell::values::AnyValue;
        let cmp = self
            .builder
            .build_call(
                cmp_fn,
                &[l_ptr.into(), l_len.into(), r_ptr.into(), r_len.into()],
                "text_cmp",
            )
            .map_err(ctx("Failed to call __text_cmp"))?
            .as_any_value_enum()
            .into_int_value();

        let pred = match operator {
            BinaryOperator::Eq => inkwell::IntPredicate::EQ,
            BinaryOperator::Ne => inkwell::IntPredicate::NE,
            BinaryOperator::Lt => inkwell::IntPredicate::SLT,
            BinaryOperator::Le => inkwell::IntPredicate::SLE,
            BinaryOperator::Gt => inkwell::IntPredicate::SGT,
            BinaryOperator::Ge => inkwell::IntPredicate::SGE,
            _ => return Err("non-comparison operator in text compare".to_string()),
        };
        let zero = cmp.get_type().const_zero();
        Ok(self
            .builder
            .build_int_compare(pred, cmp, zero, "text_cmp_res")
            .map_err(ctx("Failed to build text compare"))?
            .into())
    }
}
