//! `Text`: its built-in methods, concatenation, and comparison, over the
//! `{ data, byte_len }` representation.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower a built-in `Text` method call (`args[0]` is the `Text` receiver). Each is
    /// lowered to its `quilon-rt` intrinsic; `split` yields the `[]Text` `{ptr,i64}`
    /// struct the intrinsic builds, and `indexOf` builds an `Ok(Num)`/`NotOk` `Result`.
    ///
    /// `span` is the whole method call's span: the three methods with a fail-loud contract
    /// (`repeat`, `replace`, `replaceAll`) hand it to the runtime as a `Site`, so a violated
    /// contract reports where the call is written.
    pub(super) fn generate_text_method(
        &mut self,
        method: &str,
        args: &[Expression],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
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
            "trim" => {
                // `trim` = `trimStart` then `trimEnd` (order-independent, identical to a
                // direct both-sides trim) — composed from the two intrinsics so there is
                // no separate `__text_trim`. The extra pass/allocation is fine for trim.
                let started = call_struct(
                    self,
                    "__text_trim_start",
                    &[recv_ptr.into(), recv_len.into()],
                )?
                .into_struct_value();
                let sp = self
                    .builder
                    .build_extract_value(started, 0, "trim_mid_ptr")
                    .map_err(ctx("Failed to extract trimStart ptr"))?
                    .into_pointer_value();
                let sl = self
                    .builder
                    .build_extract_value(started, 1, "trim_mid_len")
                    .map_err(ctx("Failed to extract trimStart len"))?
                    .into_int_value();
                call_struct(self, "__text_trim_end", &[sp.into(), sl.into()])
            }
            "trimStart" => call_struct(
                self,
                "__text_trim_start",
                &[recv_ptr.into(), recv_len.into()],
            ),
            "trimEnd" => call_struct(self, "__text_trim_end", &[recv_ptr.into(), recv_len.into()]),
            "toUpper" => call_struct(self, "__text_to_upper", &[recv_ptr.into(), recv_len.into()]),
            "toLower" => call_struct(self, "__text_to_lower", &[recv_ptr.into(), recv_len.into()]),
            "split" => {
                let (sp, sl) = self.extract_text(&args[1])?;
                call_struct(
                    self,
                    "__text_split",
                    &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                )
            }
            "repeat" => {
                // `count` copies of the receiver. Passed as a Num (double): the runtime
                // rejects a negative/fractional count instead of truncating it, and the
                // checker already rejected a literal one.
                let count = self.generate_expression(&args[1])?.into_float_value();
                let site = self.site_value(span)?;
                call_struct(
                    self,
                    "__text_repeat",
                    &[recv_ptr.into(), recv_len.into(), count.into(), site.into()],
                )
            }
            "replaceAll" => {
                // Replace every occurrence. The intrinsic aborts (exit 101) on an empty
                // `from`; there is no count.
                let (fp, fl) = self.extract_text(&args[1])?;
                let (tp, tl) = self.extract_text(&args[2])?;
                let site = self.site_value(span)?;
                call_struct(
                    self,
                    "__text_replace_all",
                    &[
                        recv_ptr.into(),
                        recv_len.into(),
                        fp.into(),
                        fl.into(),
                        tp.into(),
                        tl.into(),
                        site.into(),
                    ],
                )
            }
            "replace" => {
                // Replace EXACTLY the first `count` occurrences. `count` is a Num,
                // truncated toward zero (as with array indices). The intrinsic aborts
                // (exit 101) on an empty `from`, count <= 0, or count > occurrences present
                // — a literal `count <= 0` / literal empty `from` / all-literal
                // count-exceeds were already rejected by the checker at compile time.
                let (fp, fl) = self.extract_text(&args[1])?;
                let (tp, tl) = self.extract_text(&args[2])?;
                let count = self.text_index_arg(&args[3], "replace_count")?;
                let site = self.site_value(span)?;
                call_struct(
                    self,
                    "__text_replace_n",
                    &[
                        recv_ptr.into(),
                        recv_len.into(),
                        fp.into(),
                        fl.into(),
                        tp.into(),
                        tl.into(),
                        count.into(),
                        site.into(),
                    ],
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
            "contains" => {
                let (sp, sl) = self.extract_text(&args[1])?;
                let f = self.get_intrinsic("__text_contains")?;
                let r = self
                    .builder
                    .build_call(
                        f,
                        &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                        "txt_contains",
                    )
                    .map_err(ctx("Failed to call __text_contains"))?
                    .as_any_value_enum()
                    .into_int_value();
                self.int_to_bool(r, "contains_bool")
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
        let ptr = self
            .builder
            .build_extract_value(text, 0, "txt_ptr")
            .map_err(ctx("Failed to extract text ptr"))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(text, 1, "txt_len")
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

    /// Concatenate two `Text` values into a fresh, GC-allocated, NUL-terminated
    /// buffer and return a new `{ ptr, byte_len }` struct.
    pub(super) fn generate_text_concat(
        &mut self,
        left: inkwell::values::StructValue<'ctx>,
        right: inkwell::values::StructValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i8t = self.context.i8_type();
        let i64t = self.context.i64_type();

        let field = |s: inkwell::values::StructValue<'ctx>,
                     idx: u32,
                     name: &str|
         -> Result<BasicValueEnum<'ctx>, String> {
            self.builder
                .build_extract_value(s, idx, name)
                .map_err(ctx("Failed to extract text field"))
        };
        let l_ptr = field(left, 0, "l_ptr")?.into_pointer_value();
        let l_len = field(left, 1, "l_len")?.into_int_value();
        let r_ptr = field(right, 0, "r_ptr")?.into_pointer_value();
        let r_len = field(right, 1, "r_len")?.into_int_value();

        let total = self
            .builder
            .build_int_add(l_len, r_len, "concat_len")
            .map_err(ctx("Failed to add lengths"))?;
        // +1 byte for the NUL terminator so the result is also a valid C string.
        let alloc_size = self
            .builder
            .build_int_add(total, i64t.const_int(1, false), "concat_alloc")
            .map_err(ctx("Failed to size alloc"))?;

        use inkwell::values::AnyValue;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let dest = self
            .builder
            .build_call(alloc_fn, &[alloc_size.into()], "concat_buf")
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
        let nul = unsafe {
            self.builder
                .build_gep(i8t, dest, &[total], "concat_nul")
                .map_err(ctx("Failed to offset NUL"))?
        };
        self.builder
            .build_store(nul, i8t.const_zero())
            .map_err(ctx("Failed to write NUL"))?;

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
        let (l_ptr, l_len) = self.text_fields(l.into())?;
        let (r_ptr, r_len) = self.text_fields(r.into())?;

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
