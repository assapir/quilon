//! Arrays: literals, spreads, concatenation, ranges, and the built-in array methods
//! (each lowered by inlining its lambda over an emitted loop).
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    pub(super) fn generate_array(
        &mut self,
        array_expression: &Expression,
        elements: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Arrays are represented as structs: { ptr data, i64 size }
        // This allows .size field access

        if self.current_function.is_none() {
            return Err("Global arrays not yet implemented".to_string());
        }

        // A literal containing a `<-` spread (`[<-xs, 4]`) has a runtime-determined size
        // (each spread source contributes its own `.size` elements), so it takes a
        // dedicated GC-allocating path that copies each part in order.
        if elements
            .iter()
            .any(|e| matches!(e, Expression::Spread { .. }))
        {
            return self.generate_array_spread(array_expression, elements);
        }

        let size = elements.len();

        if size == 0 {
            // Empty array - create struct with null ptr and size 0
            let ptr_type = self.context.ptr_type(AddressSpace::default());
            let i64_type = self.context.i64_type();
            let array_struct_type = self
                .context
                .struct_type(&[ptr_type.into(), i64_type.into()], false);

            let null_ptr = ptr_type.const_zero();
            let zero_size = i64_type.const_zero();

            return Ok(array_struct_type
                .const_named_struct(&[null_ptr.into(), zero_size.into()])
                .into());
        }

        // Generate all element values
        let values: Vec<BasicValueEnum> = elements
            .iter()
            .map(|e| self.generate_expression(e))
            .collect::<Result<Vec<_>, _>>()?;

        // Element type from the type oracle — the checker's UNIFIED element type across
        // every element (e.g. a sum whose variants specialize different payloads per
        // element), not the first element's own value type, which a later element may
        // have specialized further. Falls back to the first element's type when the
        // oracle has no entry (IR-only codegen tests that skip the type-check pass).
        let elem_type = match self.oracle.expression_type(array_expression) {
            Some(Type::Array(elem)) => self.value_repr_type(elem)?,
            _ => values[0].get_type(),
        };

        // Lay the elements into a GC-allocated buffer via the shared array builder — the
        // SAME mechanism used by `+` concatenation and `<-` spread. Heap (not stack)
        // allocation is essential: an array is a `{ ptr, i64 }` value whose data must
        // outlive the current frame (e.g. when the literal is returned from a function),
        // and `build_array_from_parts` already GC-allocates. Each literal element is an
        // `Inline` part (contributing one slot).
        let parts: Vec<ArrayPart<'ctx>> = values.into_iter().map(ArrayPart::Inline).collect();
        self.build_array_from_parts(elem_type, &parts)
    }

    /// Lower an array literal that contains one or more `<-` spreads (`[<-xs, 4, <-ys]`).
    /// The result size is only known at runtime (each spread contributes its source's
    /// `.size`), so the backing storage is GC-allocated to the exact total and filled
    /// left-to-right: an inline element is stored at the running offset, a spread is a
    /// flat `memcpy` of its source's data (works for any element repr — `[]Num`, `[]Text`,
    /// nested arrays — since element storage is POD in every case). The element repr type
    /// comes from the type oracle (`[]elem`), so `[]Text` spreads copy `{ptr,len}` slots
    /// correctly, not a hardcoded `f64`.
    pub(super) fn generate_array_spread(
        &mut self,
        array_expression: &Expression,
        elements: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Element repr type from the oracle (`[]elem`); fall back to f64 if the oracle
        // has no entry (IR-only codegen tests that skip type-checking).
        let elem_llvm = match self.oracle.expression_type(array_expression) {
            Some(Type::Array(elem)) => self.value_repr_type(elem)?,
            _ => self.context.f64_type().into(),
        };

        // Generate each part once, tagged spread-or-inline. A spread source lowers to a
        // `{ptr, size}` array struct; an inline element lowers to an `elem` value.
        let mut parts: Vec<ArrayPart<'ctx>> = Vec::with_capacity(elements.len());
        for elem in elements {
            if let Expression::Spread {
                expression: src, ..
            } = elem
            {
                parts.push(ArrayPart::Spread(self.generate_expression(src)?));
            } else {
                parts.push(ArrayPart::Inline(self.generate_expression(elem)?));
            }
        }

        self.build_array_from_parts(elem_llvm, &parts)
    }

    /// Lower `+` on arrays to a NEW GC-allocated array (neither operand mutated), in the
    /// three exact-type forms the checker dispatches (see `check_binary_operator`):
    ///   concat:  `[]T + []T` — every element of `left` then of `right`.
    ///   append:  `[]T + T`   — every element of `left` then the single `right`.
    ///   prepend: `T + []T`   — the single `left` then every element of `right`.
    /// Each is `[<-left, <-right]` with the single-element side `Inline` instead of
    /// `Spread`, so it reuses the spread machinery (`build_array_from_parts`) — element-repr
    /// correct for `[]Num`, `[]Text`, and nested arrays via the type oracle. The
    /// concat-vs-append form is re-derived from the operands' oracle types using the SAME
    /// `types_match` the checker used (see `check_binary_operator`), so the two sites cannot drift on
    /// what counts as "the same element type"; `[][]Num + []Num` is thus an append (`right`
    /// is one element), matching the checker.
    pub(super) fn generate_array_concat(
        &mut self,
        left: &Expression,
        right: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use crate::typechecker::types_match;

        // Classify the form (which side is the whole array to splice vs. a single element)
        // and derive the element repr, all from borrowed oracle types — no `Type` clones.
        // This borrow is scoped so it ends before the `&mut self` `generate_expression` calls.
        let (elem_llvm, left_is_array, right_is_array) = {
            let (elem, left_is_array, right_is_array) = match (
                self.oracle.expression_type(left),
                self.oracle.expression_type(right),
            ) {
                // concat `[]T + []T`: both arrays of the SAME element type.
                (Some(Type::Array(le)), Some(Type::Array(re))) if types_match(le, re) => {
                    (Some(&**le), true, true)
                }
                // append `[]T + T`: left is the array, right a single element.
                (Some(Type::Array(le)), _) => (Some(&**le), true, false),
                // prepend `T + []T`: right is the array, left a single element.
                (_, Some(Type::Array(re))) => (Some(&**re), false, true),
                // Unreachable via the routing guard in `generate_binary_operator` (it only calls
                // here when an operand's oracle type is `Array`). Defensive default.
                _ => (None, true, true),
            };
            let elem_llvm = match elem {
                Some(t) => self.value_repr_type(t)?,
                None => self.context.f64_type().into(),
            };
            (elem_llvm, left_is_array, right_is_array)
        };

        let l = self.generate_expression(left)?;
        let r = self.generate_expression(right)?;
        let part = |is_array, v| {
            if is_array {
                ArrayPart::Spread(v)
            } else {
                ArrayPart::Inline(v)
            }
        };
        self.build_array_from_parts(
            elem_llvm,
            &[part(left_is_array, l), part(right_is_array, r)],
        )
    }

    /// Build a fresh `{ptr, size}` array by laying `parts` into a GC-allocated block:
    /// sum the parts' element counts (inline = 1, spread = its `.size`), allocate the
    /// exact backing store, then fill left-to-right — an inline element is stored at the
    /// running offset, a spread source is a flat `memcpy` of its data block. Works for
    /// any element repr (`[]Num`, `[]Text`, nested arrays) since element storage is POD
    /// in every case; `elem_llvm` supplies the stride. Shared by `<-` spread literals
    /// and `+` array concatenation.
    pub(super) fn build_array_from_parts(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        parts: &[ArrayPart<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::types::BasicType;
        let i64_type = self.context.i64_type();
        let elem_size = elem_llvm
            .size_of()
            .ok_or_else(|| "array element type has no compile-time size".to_string())?;

        // Total element count: inline elements count 1 each, a spread counts its `.size`.
        let mut count = i64_type.const_zero();
        for part in parts {
            let add = match part {
                ArrayPart::Inline(_) => i64_type.const_int(1, false),
                ArrayPart::Spread(v) => self.array_size_field(*v)?,
            };
            count = self
                .builder
                .build_int_add(count, add, "concat_count")
                .map_err(ctx("Failed to sum array part count"))?;
        }

        // GC-allocate the exact `{ptr,size}` backing store (shared array helper).
        let data_ptr = self.alloc_array_data(elem_llvm, count)?;

        // Fill left-to-right, threading a running element offset.
        let memcpy_fn = self.get_intrinsic("memcpy")?;
        let mut offset = i64_type.const_zero();
        for part in parts {
            match part {
                ArrayPart::Inline(value) => {
                    let slot = unsafe {
                        self.builder
                            .build_gep(elem_llvm, data_ptr, &[offset], "concat_slot")
                            .map_err(ctx("Failed to index array slot"))?
                    };
                    self.builder
                        .build_store(slot, *value)
                        .map_err(ctx("Failed to store array element"))?;
                    offset = self
                        .builder
                        .build_int_add(offset, i64_type.const_int(1, false), "concat_off")
                        .map_err(ctx("Failed to advance array offset"))?;
                }
                ArrayPart::Spread(value) => {
                    let src_ptr = self.array_data_field(*value)?;
                    let src_size = self.array_size_field(*value)?;
                    let dest = unsafe {
                        self.builder
                            .build_gep(elem_llvm, data_ptr, &[offset], "concat_dest")
                            .map_err(ctx("Failed to index array dest"))?
                    };
                    let bytes = self
                        .builder
                        .build_int_mul(src_size, elem_size, "concat_src_bytes")
                        .map_err(ctx("Failed to size array copy"))?;
                    self.builder
                        .build_call(memcpy_fn, &[dest.into(), src_ptr.into(), bytes.into()], "")
                        .map_err(ctx("Failed to memcpy array source"))?;
                    offset = self
                        .builder
                        .build_int_add(offset, src_size, "concat_off")
                        .map_err(ctx("Failed to advance array offset"))?;
                }
            }
        }

        // Build the { ptr, size } array struct (the shared array/Text shape).
        self.array_struct(data_ptr, count)
    }

    /// Extract the data pointer (field 0) of an array `{ptr, size}` struct value.
    pub(super) fn array_data_field(
        &self,
        array: BasicValueEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let s = array.into_struct_value();
        Ok(self
            .builder
            .build_extract_value(s, 0, "arr_data")
            .map_err(ctx("Failed to extract array data ptr"))?
            .into_pointer_value())
    }

    /// Extract the size (field 1, an i64) of an array `{ptr, size}` struct value.
    pub(super) fn array_size_field(
        &self,
        array: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let s = array.into_struct_value();
        Ok(self
            .builder
            .build_extract_value(s, 1, "arr_size")
            .map_err(ctx("Failed to extract array size"))?
            .into_int_value())
    }

    /// Materialize an inclusive range `lo <- hi` into a `[]Num` (the `{ptr, size}`
    /// array shape, same as `generate_array`). The backing storage comes from the shared
    /// [`Self::alloc_array_data`], so the array may safely escape the current frame.
    ///
    /// This is the range's DEFAULT lowering — indexing, `.size`, binding to a name, and
    /// passing to a function all consume the materialized array. A range consumed
    /// directly by `.map`/`.filter`/`.reduce` (and a discarded `.each`) skips this and
    /// iterates the bounds instead — see [`Self::generate_array_method`].
    pub(super) fn generate_range(
        &mut self,
        start: &Expression,
        end: &Expression,
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if self.current_function.is_none() {
            return Err("Range must be in a function".to_string());
        }
        let f64_type = self.context.f64_type();
        let (lo, step, count) = self.range_bounds(start, end, span)?;

        let data_ptr = self.alloc_array_data(f64_type.into(), count)?;

        // Fill loop: for i in 0..count: data[i] = (f64)(lo + i*step). `array_loop` and
        // `array_struct` both allocate their stack slots in the function ENTRY block, not
        // at this insert point: a range literal is an ordinary expression that can appear
        // in a tail-recursive function body, and a raw `alloca` here would land inside the
        // TCO-lowered loop and re-allocate every iteration, overflowing the stack.
        self.array_loop(count, |this, i| {
            let value = this.range_element(lo, step, i)?;
            let element_ptr = unsafe {
                this.builder
                    .build_gep(f64_type, data_ptr, &[i], "range_elem")
                    .map_err(ctx("Failed to index range data"))?
            };
            this.builder
                .build_store(element_ptr, value)
                .map_err(ctx("Failed to store range element"))?;
            Ok(())
        })?;
        self.array_struct(data_ptr, count)
    }

    /// The lowered header of an inclusive range `lo <- hi`: both endpoints validated and
    /// converted to `i64`, folded into `(lo, step, count)` — the step is `+1`/`-1` by the
    /// runtime-decided direction (`lo <= hi` counts up), the count is `|hi - lo| + 1`.
    /// Shared by the materializing [`Self::generate_range`] and the lazy method lowerings,
    /// so endpoint validation is identical on both paths.
    ///
    /// An end that is not a whole number a `Num` holds exactly is refused at `span`. Because
    /// that bounds both ends by 2^53, the span below cannot overflow an `i64`. An end the
    /// checker already evaluated becomes a constant, so a literal range's count — and any
    /// trip count derived from it — folds to a constant too.
    pub(super) fn range_bounds(
        &mut self,
        start: &Expression,
        end: &Expression,
        span: &Span,
    ) -> Result<
        (
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        String,
    > {
        let i64_type = self.context.i64_type();

        let lo = self.range_endpoint(start, span)?;
        let hi = self.range_endpoint(end, span)?;

        // Ascending iff lo <= hi; pick step = +1 / -1.
        let ascending = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLE, lo, hi, "range_asc")
            .map_err(ctx("Failed to compare range ends"))?;
        let one = i64_type.const_int(1, false);
        let neg_one = i64_type.const_all_ones(); // -1 in two's complement
        let step = self
            .builder
            .build_select(ascending, one, neg_one, "range_step")
            .map_err(ctx("Failed to select range step"))?
            .into_int_value();
        // |hi - lo| + 1: compute the signed delta once, then pick it or its negation so the
        // span is non-negative in either direction.
        let delta = self
            .builder
            .build_int_sub(hi, lo, "range_delta")
            .map_err(ctx("Failed to subtract range ends"))?;
        let neg_delta = self
            .builder
            .build_int_neg(delta, "range_neg_delta")
            .map_err(ctx("Failed to negate range delta"))?;
        let span_abs = self
            .builder
            .build_select(ascending, delta, neg_delta, "range_span")
            .map_err(ctx("Failed to select range span"))?
            .into_int_value();
        let count = self
            .builder
            .build_int_add(span_abs, one, "range_count")
            .map_err(ctx("Failed to add range count"))?;

        Ok((lo, step, count))
    }

    /// Element `i` of a range lowered from its bounds: `(f64)(lo + i * step)` — computed,
    /// never loaded, so a lazily-consumed range needs no backing store.
    fn range_element(
        &mut self,
        lo: inkwell::values::IntValue<'ctx>,
        step: inkwell::values::IntValue<'ctx>,
        i: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i_step = self
            .builder
            .build_int_mul(i, step, "range_i_step")
            .map_err(ctx("Failed to scale range index"))?;
        let value_int = self
            .builder
            .build_int_add(lo, i_step, "range_val_i")
            .map_err(ctx("Failed to compute range element"))?;
        Ok(self
            .builder
            .build_signed_int_to_float(value_int, self.context.f64_type(), "range_val")
            .map_err(ctx("Failed to convert range element"))?
            .into())
    }

    /// One end of a range as an `i64`.
    ///
    /// A literal end the checker has already accepted becomes a constant. Anything else goes
    /// through `__range_endpoint`, which converts it under the same `check_range_endpoint`
    /// rule the checker applied — an `fptosi` here would yield poison for the values that
    /// rule rejects.
    fn range_endpoint(
        &mut self,
        end: &Expression,
        span: &Span,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        if let Some(value) = crate::ast::literal_number(end)
            && let Ok(endpoint) = quilon_rt::check_range_endpoint(value)
        {
            return Ok(self.context.i64_type().const_int(endpoint as u64, true));
        }
        let value = self.generate_expression(end)?.into_float_value();
        let site = self.site_value(span)?;
        self.call_rt_int("__range_endpoint", &[value.into(), site.into()])
    }

    /// Lower a built-in array method call (`map`/`filter`/`reduce`/`each`/`find`/`at`).
    /// `args[0]` is the receiver array; the rest are the method's arguments (a lambda
    /// for the higher-order forms, a `Num` index for `at`). A method's lambda argument is
    /// a deliberate inline specialization of the general lambda lowering: rather than
    /// emitting a closure value, its body is INLINED into the generated loop body per
    /// element (`inline_lambda`) — cheaper, and it sidesteps the unsupported
    /// higher-order-value path. The element LLVM type comes from the type oracle (the
    /// receiver's `[]elem` element type), so `[]Text`/`[]Num`/... all work.
    pub(super) fn generate_array_method(
        &mut self,
        method: &str,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let recv = &args[0];

        // Lazy range lowering: when the receiver is SYNTACTICALLY a `lo <- hi` expression,
        // `.map`/`.filter`/`.reduce` iterate the bounds directly instead of materializing
        // the `[]Num` — `.reduce` allocates nothing, `.map`/`.filter` allocate only their
        // result. Endpoint validation is shared with the materializing path
        // (`range_bounds`), so the same errors fire at the same site. `.each` takes the
        // same lazy path only in discarded-statement position (`lower_discarded_range_each`
        // in `generate_block`): an `.each` whose value is USED — chained, bound, returned —
        // yields its receiver (Decision 19), so it must materialize the array below.
        // ponytail: only the FIRST method of a chain is lazy — later links consume the
        // (already result-sized) arrays it produces; fuse whole map/filter/each chains
        // end-to-end if profiles ever demand it.
        if let Expression::Range { start, end, span } = recv
            && matches!(method, "map" | "filter" | "reduce")
        {
            let (lo, step, count) = self.range_bounds(start, end, span)?;
            let source = ElementSource::Range { lo, step };
            let f64_llvm: BasicTypeEnum<'ctx> = self.context.f64_type().into();
            return match method {
                "map" => self.array_map(&args[1], &Type::Num, f64_llvm, source, count),
                "filter" => self.array_filter(&args[1], &Type::Num, f64_llvm, source, count),
                "reduce" => {
                    self.array_reduce(&args[1], &args[2], &Type::Num, f64_llvm, source, count)
                }
                _ => unreachable!("guarded by the matches! above"),
            };
        }

        // Element type of the receiver array, from the oracle: `[]elem`.
        let elem_qty = match self.oracle.expression_type(recv) {
            Some(Type::Array(e)) => (**e).clone(),
            _ => return Err(format!("array method `{method}` on a non-array receiver")),
        };
        let elem_llvm = self.value_repr_type(&elem_qty)?;
        let (array_val, data_ptr, size) = self.extract_array(recv)?;
        let source = ElementSource::Memory(data_ptr);

        match method {
            "map" => self.array_map(&args[1], &elem_qty, elem_llvm, source, size),
            "filter" => self.array_filter(&args[1], &elem_qty, elem_llvm, source, size),
            "reduce" => self.array_reduce(&args[1], &args[2], &elem_qty, elem_llvm, source, size),
            "each" => {
                self.array_each(&args[1], &elem_qty, elem_llvm, source, size)?;
                // Decision 19: a Unit-bodied method returns its receiver — `.each` yields
                // the array itself so it chains. Re-emit the (already-evaluated) struct.
                Ok(array_val)
            }
            "find" => self.array_find(&args[1], &elem_qty, elem_llvm, data_ptr, size),
            "at" => self.array_at(&args[1], elem_llvm, data_ptr, size),
            other => Err(format!("unknown array method `{other}`")),
        }
    }

    /// A discarded-value `(lo <- hi).each(f)` statement, lowered lazily: the loop runs
    /// over the range's bounds and NOTHING is allocated. Only `generate_block` calls this,
    /// and only for a non-final statement — the one position where `.each`'s value (its
    /// receiver, per Decision 19) is guaranteed unobserved, so skipping the
    /// materialization cannot change behavior. Returns `false` (having emitted nothing)
    /// when `expression` is not that exact shape; the caller then lowers it normally.
    pub(super) fn lower_discarded_range_each(
        &mut self,
        expression: &Expression,
    ) -> Result<bool, String> {
        let Expression::Call {
            function,
            arguments,
            member_call: true,
            ..
        } = expression
        else {
            return Ok(false);
        };
        let Expression::Identifier { name, .. } = &**function else {
            return Ok(false);
        };
        if name != "each" || arguments.len() != 2 {
            return Ok(false);
        }
        let Expression::Range { start, end, span } = &arguments[0] else {
            return Ok(false);
        };
        // Mirror the dispatch guard in `generate_call`: only an oracle-confirmed array
        // receiver reaches the built-in `each` (an unchecked IR-only run stays eager).
        if !matches!(
            self.oracle.expression_type(&arguments[0]),
            Some(Type::Array(_))
        ) {
            return Ok(false);
        }
        let (lo, step, count) = self.range_bounds(start, end, span)?;
        let f64_llvm: BasicTypeEnum<'ctx> = self.context.f64_type().into();
        self.array_each(
            &arguments[1],
            &Type::Num,
            f64_llvm,
            ElementSource::Range { lo, step },
            count,
        )?;
        Ok(true)
    }

    /// Evaluate an array expression and break it into `(struct_value, data_ptr, size_i64)`.
    /// The array ABI is the shared `{ ptr data, i64 size }` struct; this stores it to a
    /// temporary alloca to GEP out the two fields.
    pub(super) fn extract_array(
        &mut self,
        array_expression: &Expression,
    ) -> Result<
        (
            BasicValueEnum<'ctx>,
            PointerValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        String,
    > {
        let array_val = self.generate_expression(array_expression)?;
        let struct_ty = self.ptr_len_struct_type();
        let alloca = self.create_entry_block_alloca("am_array", struct_ty.into())?;
        self.builder
            .build_store(alloca, array_val)
            .map_err(ctx("Failed to store array"))?;
        let data_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "am_data_field")
            .map_err(ctx("Failed to GEP data field"))?;
        let data_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                data_field,
                "am_data",
            )
            .map_err(ctx("Failed to load data ptr"))?
            .into_pointer_value();
        let size_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 1, "am_size_field")
            .map_err(ctx("Failed to GEP size field"))?;
        let size = self
            .builder
            .build_load(self.context.i64_type(), size_field, "am_size")
            .map_err(ctx("Failed to load size"))?
            .into_int_value();
        Ok((array_val, data_ptr, size))
    }

    /// GC-allocate a `{ ptr, size }` array of `count` elements of `elem_llvm`, returning
    /// the data pointer. The caller fills it, then builds the struct via `array_struct`.
    ///
    /// The element count and the element size go to the runtime as they are, rather than
    /// as a product computed here: `__alloc_array` multiplies them under an overflow
    /// check, which an `i64` `mul` in the emitted code cannot do.
    pub(super) fn alloc_array_data(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        count: inkwell::values::IntValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let elem_size = elem_llvm
            .size_of()
            .ok_or_else(|| "array element type has no compile-time size".to_string())?;
        let alloc = self.get_intrinsic("__alloc_array")?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(alloc, &[count.into(), elem_size.into()], "am_alloc")
            .map_err(ctx("Failed to allocate array"))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// Build the `{ ptr, i64 }` array struct value from a data pointer and element count.
    pub(super) fn array_struct(
        &mut self,
        data_ptr: PointerValue<'ctx>,
        count: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let struct_ty = self.ptr_len_struct_type();
        let alloca = self.create_entry_block_alloca("am_out", struct_ty.into())?;
        let ptr_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "am_out_ptr")
            .map_err(ctx("Failed to GEP out ptr"))?;
        self.builder
            .build_store(ptr_field, data_ptr)
            .map_err(ctx("Failed to store out ptr"))?;
        let size_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 1, "am_out_size")
            .map_err(ctx("Failed to GEP out size"))?;
        self.builder
            .build_store(size_field, count)
            .map_err(ctx("Failed to store out size"))?;
        self.builder
            .build_load(struct_ty, alloca, "am_out")
            .map_err(ctx("Failed to load out struct"))
    }

    /// Element `i` of an array method's receiver: loaded from a materialized array's
    /// backing store, or computed from a lazily-lowered range's bounds.
    fn source_element(
        &mut self,
        source: ElementSource<'ctx>,
        elem_llvm: BasicTypeEnum<'ctx>,
        i: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match source {
            ElementSource::Memory(data_ptr) => self.load_element(data_ptr, elem_llvm, i),
            ElementSource::Range { lo, step } => self.range_element(lo, step, i),
        }
    }

    /// Load `data_ptr[i]` as a value of `elem_llvm` (the array element representation).
    pub(super) fn load_element(
        &mut self,
        data_ptr: PointerValue<'ctx>,
        elem_llvm: BasicTypeEnum<'ctx>,
        i: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr = unsafe {
            self.builder
                .build_gep(elem_llvm, data_ptr, &[i], "am_elem_ptr")
                .map_err(ctx("Failed to GEP element"))?
        };
        self.builder
            .build_load(elem_llvm, ptr, "am_elem")
            .map_err(ctx("Failed to load element"))
    }

    /// Inline a lambda body with its parameters bound to `arg_values`. An array method's
    /// lambda is lowered inline (not as a closure value): each parameter is bound to a
    /// freshly-stored value (an alloca, like a loop variable) and the body is emitted in
    /// the current block. Saves/restores any shadowed bindings of the same names, so an
    /// inline never leaks the parameter binding past its use (and nesting is safe).
    /// `arg_values` carries each argument's Quilon type for overload mangling in the body.
    pub(super) fn inline_lambda(
        &mut self,
        lambda: &Expression,
        arg_values: &[(BasicValueEnum<'ctx>, Type)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Expression::Lambda {
            parameters, body, ..
        } = lambda
        else {
            return Err("array method expects a lambda argument".to_string());
        };
        if parameters.len() != arg_values.len() {
            return Err(format!(
                "lambda expects {} parameter(s), got {} argument(s)",
                parameters.len(),
                arg_values.len()
            ));
        }
        // Save shadowed bindings to restore after inlining.
        let mut saved: Vec<SavedBinding<'ctx>> = Vec::with_capacity(parameters.len());
        for (parameter, (value, qty)) in parameters.iter().zip(arg_values) {
            let alloca = self.create_entry_block_alloca(&parameter.name, value.get_type())?;
            self.builder
                .build_store(alloca, *value)
                .map_err(ctx("Failed to store lambda parameter"))?;
            saved.push((
                parameter.name.clone(),
                self.variables.get(&parameter.name).copied(),
                self.var_types.get(&parameter.name).cloned(),
            ));
            self.variables
                .insert(parameter.name.clone(), (alloca, value.get_type()));
            self.var_types.insert(parameter.name.clone(), qty.clone());
        }
        let result = self.generate_expression(body);
        // Restore shadowed bindings.
        for (name, prev_var, prev_ty) in saved {
            match prev_var {
                Some(v) => {
                    self.variables.insert(name.clone(), v);
                }
                None => {
                    self.variables.remove(&name);
                }
            }
            match prev_ty {
                Some(t) => {
                    self.var_types.insert(name, t);
                }
                None => {
                    self.var_types.remove(&name);
                }
            }
        }
        result
    }

    /// `arr.map(f)` — a new array whose element `i` is `f(arr[i])`. The result element
    /// type is the lambda body's type (from the oracle), so `map` may change the element
    /// type (e.g. `[]Num -> []Text`).
    pub(super) fn array_map(
        &mut self,
        lambda: &Expression,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        source: ElementSource<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_llvm = match self.lambda_body_repr(lambda) {
            Some(r) => r?,
            None => elem_llvm,
        };
        let out_ptr = self.alloc_array_data(result_llvm, size)?;
        self.array_loop(size, |this, i| {
            let elem = this.source_element(source, elem_llvm, i)?;
            let mapped = this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            let dst = unsafe {
                this.builder
                    .build_gep(result_llvm, out_ptr, &[i], "map_dst")
                    .map_err(ctx("Failed to GEP map dst"))?
            };
            this.builder
                .build_store(dst, mapped)
                .map_err(ctx("Failed to store mapped"))?;
            Ok(())
        })?;
        self.array_struct(out_ptr, size)
    }

    /// `arr.filter(pred)` — a new array of the elements for which `pred(elem)` is true,
    /// in order. The output buffer is sized to the input (worst case, all kept); the
    /// result struct reports the actual kept count.
    pub(super) fn array_filter(
        &mut self,
        lambda: &Expression,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        source: ElementSource<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i64t = self.context.i64_type();
        let out_ptr = self.alloc_array_data(elem_llvm, size)?;
        let count_ptr = self.create_entry_block_alloca("filter_count", i64t.into())?;
        self.builder
            .build_store(count_ptr, i64t.const_zero())
            .map_err(ctx("Failed to init filter count"))?;
        self.array_loop(size, |this, i| {
            let elem = this.source_element(source, elem_llvm, i)?;
            let keep = this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            let keep_bool = this.value_to_boolean(keep)?;
            let function = this.current_function.unwrap();
            let keep_bb = this.context.append_basic_block(function, "filter_keep");
            let cont_bb = this.context.append_basic_block(function, "filter_cont");
            this.builder
                .build_conditional_branch(keep_bool, keep_bb, cont_bb)
                .map_err(ctx("Failed to branch filter"))?;
            this.builder.position_at_end(keep_bb);
            let count = this
                .builder
                .build_load(i64t, count_ptr, "filter_n")
                .map_err(ctx("Failed to load filter count"))?
                .into_int_value();
            let dst = unsafe {
                this.builder
                    .build_gep(elem_llvm, out_ptr, &[count], "filter_dst")
                    .map_err(ctx("Failed to GEP filter dst"))?
            };
            this.builder
                .build_store(dst, elem)
                .map_err(ctx("Failed to store kept"))?;
            let next = this
                .builder
                .build_int_add(count, i64t.const_int(1, false), "filter_next")
                .map_err(ctx("Failed to inc filter count"))?;
            this.builder
                .build_store(count_ptr, next)
                .map_err(ctx("Failed to store filter count"))?;
            this.builder
                .build_unconditional_branch(cont_bb)
                .map_err(ctx("Failed to branch filter cont"))?;
            this.builder.position_at_end(cont_bb);
            Ok(())
        })?;
        let count = self
            .builder
            .build_load(i64t, count_ptr, "filter_total")
            .map_err(ctx("Failed to load filter total"))?
            .into_int_value();
        self.array_struct(out_ptr, count)
    }

    /// `arr.reduce(init, (acc, x) => ...)` — fold left, threading `acc` (initialized to
    /// `init`) through the lambda for each element. The result is the final accumulator.
    pub(super) fn array_reduce(
        &mut self,
        init: &Expression,
        lambda: &Expression,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        source: ElementSource<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let init_val = self.generate_expression(init)?;
        let acc_qty = self
            .oracle
            .expression_type(init)
            .ok_or_else(|| {
                format!(
                    "internal error: no oracle type recorded for `reduce` initial value at {:?}",
                    init.span()
                )
            })?
            .clone();
        let acc_ptr = self.create_entry_block_alloca("reduce_acc", init_val.get_type())?;
        self.builder
            .build_store(acc_ptr, init_val)
            .map_err(ctx("Failed to init acc"))?;
        let acc_llvm = init_val.get_type();
        self.array_loop(size, |this, i| {
            let elem = this.source_element(source, elem_llvm, i)?;
            let acc = this
                .builder
                .build_load(acc_llvm, acc_ptr, "reduce_load")
                .map_err(ctx("Failed to load acc"))?;
            let next =
                this.inline_lambda(lambda, &[(acc, acc_qty.clone()), (elem, elem_qty.clone())])?;
            this.builder
                .build_store(acc_ptr, next)
                .map_err(ctx("Failed to store acc"))?;
            Ok(())
        })?;
        self.builder
            .build_load(acc_llvm, acc_ptr, "reduce_result")
            .map_err(ctx("Failed to load reduce result"))
    }

    /// `arr.each(f)` — run `f` on every element for side effects; the result is ignored
    /// (the receiver is returned by the caller).
    pub(super) fn array_each(
        &mut self,
        lambda: &Expression,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        source: ElementSource<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<(), String> {
        self.array_loop(size, |this, i| {
            let elem = this.source_element(source, elem_llvm, i)?;
            this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            Ok(())
        })?;
        Ok(())
    }

    /// `arr.find(pred)` — `Ok(elem)` for the first element satisfying `pred`, else
    /// `NotOk($)`. Both arms produce the SAME `{ i8 tag, elem }` struct so the result
    /// has one type; the `NotOk` payload slot is zeroed (never read).
    pub(super) fn array_find(
        &mut self,
        lambda: &Expression,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_ty = self.result_struct_type(elem_llvm);
        let result_ptr = self.create_entry_block_alloca("find_result", result_ty.into())?;
        // Default: NotOk (tag 1, zeroed payload).
        let not_ok = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm))?;
        self.builder
            .build_store(result_ptr, not_ok)
            .map_err(ctx("Failed to init find result"))?;

        let function = self.current_function.unwrap();
        let done_bb = self.context.append_basic_block(function, "find_done");

        // Loop with an early exit to `done_bb` on the first match.
        let i64t = self.context.i64_type();
        let counter = self.create_entry_block_alloca("find_i", i64t.into())?;
        self.builder
            .build_store(counter, i64t.const_zero())
            .map_err(ctx("Failed to init find counter"))?;
        let header = self.context.append_basic_block(function, "find_header");
        let body = self.context.append_basic_block(function, "find_body");
        self.builder
            .build_unconditional_branch(header)
            .map_err(ctx("Failed to branch find header"))?;
        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64t, counter, "find_iv")
            .map_err(ctx("Failed to load find counter"))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, size, "find_cond")
            .map_err(ctx("Failed to compare find counter"))?;
        self.builder
            .build_conditional_branch(cond, body, done_bb)
            .map_err(ctx("Failed to branch find body"))?;
        self.builder.position_at_end(body);
        let elem = self.load_element(data_ptr, elem_llvm, i)?;
        let matched = self.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
        let matched_bool = self.value_to_boolean(matched)?;
        let found_bb = self.context.append_basic_block(function, "find_found");
        let next_bb = self.context.append_basic_block(function, "find_next");
        self.builder
            .build_conditional_branch(matched_bool, found_bb, next_bb)
            .map_err(ctx("Failed to branch find match"))?;
        // Found: store Ok(elem) and jump to done. `body` dominates `found_bb`, so the
        // `elem` already loaded above is in scope here — no need to reload it.
        self.builder.position_at_end(found_bb);
        let ok = self.build_result(elem_llvm, "Ok", elem)?;
        self.builder
            .build_store(result_ptr, ok)
            .map_err(ctx("Failed to store find Ok"))?;
        self.builder
            .build_unconditional_branch(done_bb)
            .map_err(ctx("Failed to branch find done"))?;
        // Next iteration.
        self.builder.position_at_end(next_bb);
        let inc = self
            .builder
            .build_int_add(i, i64t.const_int(1, false), "find_inc")
            .map_err(ctx("Failed to inc find counter"))?;
        self.builder
            .build_store(counter, inc)
            .map_err(ctx("Failed to store find counter"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(ctx("Failed to loop find"))?;

        self.builder.position_at_end(done_bb);
        self.builder
            .build_load(result_ty, result_ptr, "find_value")
            .map_err(ctx("Failed to load find result"))
    }

    /// `0.0 <= idx_f < (double)size` — the shared bounds primitive behind both raw
    /// `arr[i]` and `arr.at(n)`. Computed entirely on the **f64** index, BEFORE any
    /// `fptosi`, so an invalid index can never reach the poison-producing conversion.
    /// Both compares are ORDERED (`OGE`/`OLT`), so a NaN index fails them and needs no
    /// separate check. A fractional in-range index is deliberately valid: the conversion
    /// truncates toward zero (documented — the language has no integer division, so index
    /// arithmetic legitimately produces fractional values).
    pub(super) fn index_in_bounds(
        &mut self,
        idx_f: inkwell::values::FloatValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let f64t = self.context.f64_type();
        let size_f = self
            .builder
            .build_signed_int_to_float(size, f64t, "size_f")
            .map_err(ctx("Failed to convert size"))?;
        let ge0 = self
            .builder
            .build_float_compare(
                inkwell::FloatPredicate::OGE,
                idx_f,
                f64t.const_zero(),
                "idx_ge0",
            )
            .map_err(ctx("Failed to compare index lower bound"))?;
        let lt = self
            .builder
            .build_float_compare(inkwell::FloatPredicate::OLT, idx_f, size_f, "idx_lt")
            .map_err(ctx("Failed to compare index upper bound"))?;
        self.builder
            .build_and(ge0, lt, "idx_in_bounds")
            .map_err(ctx("Failed to and index bounds"))
    }

    /// `arr.at(n)` — `Ok(arr[n])` if `0 <= n < size`, else `NotOk($)` (safe index).
    pub(super) fn array_at(
        &mut self,
        index: &Expression,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let idx_f = self.generate_expression(index)?.into_float_value();
        // Bounds-check on the f64 BEFORE converting: `fptosi` of a NaN or out-of-range
        // value is poison, so the old convert-then-compare order branched on poison for
        // `at(0/0)`. The conversion in the `Ok` payload only executes on the in-bounds path.
        let in_bounds = self.index_in_bounds(idx_f, size)?;
        self.build_conditional_result(in_bounds, elem_llvm, "at", |this| {
            let idx = this
                .builder
                .build_float_to_signed_int(idx_f, this.context.i64_type(), "at_idx")
                .map_err(ctx("Failed to convert at index"))?;
            this.load_element(data_ptr, elem_llvm, idx)
        })
    }

    /// The LLVM value-representation type of a lambda's body (its inferred result type,
    /// from the oracle), if known — used by `map` to size the output array. `None` when
    /// the oracle has no entry (IR-only tests), so the caller falls back.
    pub(super) fn lambda_body_repr(
        &self,
        lambda: &Expression,
    ) -> Option<Result<BasicTypeEnum<'ctx>, String>> {
        let Expression::Lambda { body, .. } = lambda else {
            return None;
        };
        self.oracle
            .expression_type(body)
            .map(|t| self.value_repr_type(t))
    }

    /// Emit a counted `for i in 0..size` loop, calling `body(self, i)` in the loop body
    /// (the builder is positioned in the body block). On return the builder sits in the
    /// loop's exit block. Shared scaffolding for the array methods that visit every
    /// element in order (`map`/`filter`/`reduce`/`each`). `find` rolls its own loop (it
    /// needs an early exit).
    pub(super) fn array_loop(
        &mut self,
        size: inkwell::values::IntValue<'ctx>,
        mut body: impl FnMut(&mut Self, inkwell::values::IntValue<'ctx>) -> Result<(), String>,
    ) -> Result<(), String> {
        let i64t = self.context.i64_type();
        let function = self.current_function.unwrap();
        let counter = self.create_entry_block_alloca("am_i", i64t.into())?;
        self.builder
            .build_store(counter, i64t.const_zero())
            .map_err(ctx("Failed to init loop counter"))?;
        let header = self.context.append_basic_block(function, "am_header");
        let body_bb = self.context.append_basic_block(function, "am_body");
        let exit = self.context.append_basic_block(function, "am_exit");
        self.builder
            .build_unconditional_branch(header)
            .map_err(ctx("Failed to branch loop header"))?;
        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64t, counter, "am_iv")
            .map_err(ctx("Failed to load loop counter"))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, size, "am_cond")
            .map_err(ctx("Failed to compare loop counter"))?;
        self.builder
            .build_conditional_branch(cond, body_bb, exit)
            .map_err(ctx("Failed to branch loop body"))?;
        self.builder.position_at_end(body_bb);
        body(self, i)?;
        let inc = self
            .builder
            .build_int_add(i, i64t.const_int(1, false), "am_inc")
            .map_err(ctx("Failed to inc loop counter"))?;
        self.builder
            .build_store(counter, inc)
            .map_err(ctx("Failed to store loop counter"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(ctx("Failed to loop"))?;
        self.builder.position_at_end(exit);
        Ok(())
    }
}
