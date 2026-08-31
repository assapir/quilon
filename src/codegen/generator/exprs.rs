//! The expression dispatcher and the general-purpose expression forms: operators,
//! conditionals, blocks, and indexing.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower `expression` to a value, then force it in place if the deferred-taint pass marked this
    /// span a force site (a deferred value sitting where a strict primitive reads it). The force
    /// is the ONE seam where deferral becomes visible to codegen; everywhere else a deferred value
    /// is threaded as its ordinary type. A deferred value is a `Text` (`@readStdin`) or a `Result`
    /// (`@tcpRequest`); each force helper acts only on its own representation and passes everything
    /// else — including an already-ready value — straight through, so chaining them forces exactly
    /// the one that applies. A non-force-site span — every expression in a pure program — lowers
    /// to the call alone, with no force wrapper around it.
    /// A `Text` whose bytes are known while emitting, so they become a global constant.
    /// Backs both string literals and the `core.info` members.
    pub(super) fn build_text_constant(
        &mut self,
        value: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let global = self
            .builder
            .build_global_string_ptr(value, "str")
            .map_err(ctx("Failed to build string"))?;
        let data_ptr = global.as_pointer_value();
        let len = self.context.i64_type().const_int(value.len() as u64, false);
        let text_ty = self.ptr_len_struct_type();
        let with_ptr = self
            .builder
            .build_insert_value(text_ty.get_undef(), data_ptr, 0, "text_ptr")
            .map_err(ctx("Failed to insert text ptr"))?
            .into_struct_value();
        let text = self
            .builder
            .build_insert_value(with_ptr, len, 1, "text_len")
            .map_err(ctx("Failed to insert text len"))?
            .into_struct_value();
        Ok(text.into())
    }

    pub(super) fn generate_expression(
        &mut self,
        expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let value = self.generate_expression_inner(expression)?;
        if self.defer.is_force_site(expression.span()) {
            let value = self.force_deferred_text(value)?;
            return self.force_deferred_result(value);
        }
        Ok(value)
    }

    fn generate_expression_inner(
        &mut self,
        expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Attribute the instructions this expression lowers to its source location, so the
        // DWARF line table maps generated code back to the `.qn` line (no-op without debug).
        self.set_debug_loc(expression.span());
        match expression {
            Expression::Number { value, .. } => {
                // For now, use f64 for all numbers
                Ok(self.context.f64_type().const_float(*value).into())
            }

            Expression::String { value, .. } => self.build_text_constant(value),

            Expression::Interpolation { parts, .. } => self.generate_interpolation(parts),

            Expression::Bool { value, .. } => Ok(self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into()),

            // The unit value `$`: a zero `i8` placeholder. The value is never
            // inspected; it just needs a concrete, single-inhabitant representation.
            Expression::Unit { .. } => Ok(self.unit_value().into()),

            Expression::Identifier { name, .. } => {
                // A bare nullary sum-type constructor (e.g. `Red`) builds its tagged
                // struct here. Payload-carrying constructors are calls, handled above.
                // (We only treat it as a constructor when it isn't a bound variable.)
                if !self.variables.contains_key(name)
                    && let Some((tag, type_name)) = self.sum_variants.get(name).cloned()
                {
                    return self.generate_sum_constructor(tag, &type_name, &[]);
                }
                // Local binding (function-scoped alloca) first.
                if let Some((ptr, ty)) = self.variables.get(name) {
                    return self
                        .builder
                        .build_load(*ty, *ptr, name)
                        .map_err(ctx("Failed to build load"));
                }
                // Otherwise a top-level/module global constant (e.g. core.io's
                // `stdout`/`stderr`, or any top-level `name = <const>`).
                if let Some(global) = self.module.get_global(name) {
                    let ty = global
                        .get_initializer()
                        .map(|v| v.get_type())
                        .unwrap_or_else(|| self.context.f64_type().into());
                    return self
                        .builder
                        .build_load(ty, global.as_pointer_value(), name)
                        .map_err(ctx("Failed to build load global"));
                }
                Err(format!("Undefined variable: {}", name))
            }

            Expression::BinaryOperator {
                left,
                operator,
                right,
                ..
            } => self.generate_binary_operator(left, *operator, right),

            Expression::UnaryOperator {
                operator,
                expression,
                ..
            } => self.generate_unary_operator(*operator, expression),

            Expression::Call {
                function,
                arguments,
                member_call,
                span,
            } => self.generate_call(function, arguments, *member_call, span),

            Expression::Lambda {
                parameters,
                return_type,
                body,
                ..
            } => self.generate_lambda(parameters, return_type.as_ref(), body),

            Expression::If {
                condition,
                then,
                else_,
                ..
            } => self.generate_if(condition, then, else_),

            Expression::Block { statements, span } => self.generate_block(statements, span),

            Expression::Array { elements, .. } => self.generate_array(expression, elements),

            Expression::MapLiteral { entries, .. } => {
                self.generate_map_literal(expression, entries)
            }

            Expression::SetLiteral { elements, .. } => {
                self.generate_set_literal(expression, elements)
            }

            Expression::Record { fields, .. } => {
                self.generate_record_expression(expression, fields)
            }

            // A bare spread never survives to codegen on its own — the parser only
            // produces one as an element of an array literal or a field of a record
            // literal, where `generate_array` / `generate_record_expression` consume it.
            Expression::Spread { .. } => {
                Err("spread `<-` is only valid inside an array or record literal".to_string())
            }

            Expression::Constructor {
                type_name, fields, ..
            } => {
                // A `<-source` entry fills the fields it is not overriding, exactly as in
                // an anonymous literal — and the update lowering already builds its result
                // from the whole literal's oracle type, which here is this named type, so
                // the slots land in declaration order with the type's methods intact.
                if fields
                    .iter()
                    .any(|(_, v)| matches!(v, Expression::Spread { .. }))
                {
                    return self.generate_record_update(expression, fields);
                }
                // A named-type instance has the same struct representation as a record,
                // but its field SLOTS follow the type's DECLARATION order — which is the
                // order `record_types` and the type oracle use to index/GEP fields later.
                // The constructor call may list fields in any order, so reorder them to
                // declaration order before lowering; otherwise a later `obj.field` read
                // would GEP the wrong slot (silent corruption once fields differ in type).
                let ordered = self.constructor_fields_in_declaration_order(type_name, fields);
                self.generate_record(&ordered)
            }

            Expression::FieldAccess {
                expression, field, ..
            } => self.generate_field_access(expression, field),

            Expression::FieldAssign { target, value, .. } => {
                self.generate_field_assign(target, value)
            }

            Expression::Index {
                expression: array,
                index,
                ..
            } => self.generate_index(expression, array, index),

            Expression::Match {
                expression: scrutinee,
                arms,
                ..
            } => self.generate_match(expression, scrutinee, arms),

            Expression::Range { start, end, span } => self.generate_range(start, end, span),

            // `left |> right` desugars to a call with `left` as the first arg
            // (must match the type checker's desugaring exactly).
            Expression::Pipeline { left, right, span } => {
                let call = Expression::desugar_pipeline(left, right, span);
                self.generate_expression(&call)
            }
        }
    }

    /// Emit the force of a possibly-deferred `Text`: if `value`'s length field is the deferred
    /// sentinel (`-1`), its first field is a promise pointer — call `__force_text` to park
    /// until the value is ready and read its bytes (memoized); otherwise pass the ready `Text`
    /// straight through. A runtime branch, because a force site fed by a `?`/ternary can be
    /// deferred on one path and ready on the other. Non-`Text` values (never deferred in this
    /// tier) pass through untouched.
    fn force_deferred_text(
        &mut self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let text_ty = self.ptr_len_struct_type();
        let BasicValueEnum::StructValue(deferred) = value else {
            return Ok(value);
        };
        if deferred.get_type() != text_ty {
            return Ok(value);
        }

        let length = self
            .builder
            .build_extract_value(deferred, 1, "deferred_len")
            .map_err(ctx("Failed to read deferred length"))?
            .into_int_value();
        let sentinel = self
            .context
            .i64_type()
            .const_int(quilon_rt::deferred::DEFERRED_SENTINEL as u64, true);
        let is_deferred = self
            .builder
            .build_int_compare(inkwell::IntPredicate::EQ, length, sentinel, "is_deferred")
            .map_err(ctx("Failed to test deferred sentinel"))?;

        self.emit_force_branch(value, is_deferred, |generator| {
            // Deferred: extract the promise pointer and force it (park-until-ready, memoized).
            let promise = generator
                .builder
                .build_extract_value(deferred, 0, "deferred_promise")
                .map_err(ctx("Failed to read deferred promise"))?
                .into_pointer_value();
            let force_fn = generator.get_intrinsic("__force_text")?;
            Self::call_result_to_basic(
                generator
                    .builder
                    .build_call(force_fn, &[promise.into()], "forced")
                    .map_err(ctx("Failed to call __force_text"))?,
            )
        })
    }

    /// Emit the force of a possibly-deferred `Result`: if `value`'s tag field is the deferred
    /// tag, its slot's `data` field is a promise pointer — call `__force_result` to park until
    /// the value is ready and read its `{ tag, slot }` (memoized); otherwise pass the ready
    /// `Result` straight through. A runtime branch, because a force site fed by a `?`/ternary can
    /// be deferred on one path and ready on the other. Non-`Result` values pass through untouched,
    /// so this is a no-op on the `Text` deferral path and on all pure code.
    fn force_deferred_result(
        &mut self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_ty = self.sum_struct_type("Result");
        let BasicValueEnum::StructValue(deferred) = value else {
            return Ok(value);
        };
        if deferred.get_type() != result_ty {
            return Ok(value);
        }

        let tag = self
            .builder
            .build_extract_value(deferred, 0, "deferred_result_tag")
            .map_err(ctx("Failed to read deferred Result tag"))?
            .into_int_value();
        let sentinel = self
            .context
            .i8_type()
            .const_int(quilon_rt::deferred::DEFERRED_RESULT_TAG as u8 as u64, false);
        let is_deferred = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                tag,
                sentinel,
                "is_deferred_result",
            )
            .map_err(ctx("Failed to test deferred Result tag"))?;

        self.emit_force_branch(value, is_deferred, |generator| {
            // Deferred: the promise pointer is the slot's `data` field (slot is `{ptr,i64}`, field
            // 1 of the Result; its pointer is sub-field 0). Force it (park-until-ready, memoized).
            let slot = generator
                .builder
                .build_extract_value(deferred, 1, "deferred_result_slot")
                .map_err(ctx("Failed to read deferred Result slot"))?
                .into_struct_value();
            let promise = generator
                .builder
                .build_extract_value(slot, 0, "deferred_result_promise")
                .map_err(ctx("Failed to read deferred Result promise"))?
                .into_pointer_value();
            let force_fn = generator.get_intrinsic("__force_result")?;
            // A `Result` (24 bytes) crosses the FFI via an out-pointer, not an aggregate return.
            let out = generator.create_entry_block_alloca("force_result_out", result_ty.into())?;
            generator
                .builder
                .build_call(force_fn, &[out.into(), promise.into()], "")
                .map_err(ctx("Failed to call __force_result"))?;
            generator
                .builder
                .build_load(result_ty, out, "forced_result")
                .map_err(ctx("Failed to load forced Result"))
        })
    }

    /// The shared force scaffolding: given a runtime `is_deferred` flag, branch to a `force` block
    /// where `emit_forced` builds the forced value, and phi it back with the untouched ready
    /// `value` at the join. The force-site's block/phi plumbing lives here once; each caller
    /// supplies only its own discriminant test (above) and its representation-specific force body.
    fn emit_force_branch(
        &mut self,
        value: BasicValueEnum<'ctx>,
        is_deferred: inkwell::values::IntValue<'ctx>,
        emit_forced: impl FnOnce(&mut Self) -> Result<BasicValueEnum<'ctx>, String>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ready_block = self.builder.get_insert_block().unwrap();
        let function = ready_block.get_parent().unwrap();
        let force_block = self.context.append_basic_block(function, "force");
        let cont_block = self.context.append_basic_block(function, "force_cont");
        self.builder
            .build_conditional_branch(is_deferred, force_block, cont_block)
            .map_err(ctx("Failed to branch on deferred value"))?;

        self.builder.position_at_end(force_block);
        let forced = emit_forced(self)?;
        let force_end_block = self.builder.get_insert_block().unwrap();
        self.builder
            .build_unconditional_branch(cont_block)
            .map_err(ctx("Failed to branch out of force"))?;

        // Join: the ready value (unchanged) or the forced value.
        self.builder.position_at_end(cont_block);
        let phi = self
            .builder
            .build_phi(value.get_type(), "forced_or_ready")
            .map_err(ctx("Failed to build force phi"))?;
        phi.add_incoming(&[(&value, ready_block), (&forced, force_end_block)]);
        Ok(phi.as_basic_value())
    }

    pub(super) fn generate_binary_operator(
        &mut self,
        left: &Expression,
        operator: BinaryOperator,
        right: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A USER operator overload (e.g. `+`/`==` on a record type) lowers to a direct
        // call to its mangled function — the operator is just a named overload set.
        // Built-in operators (Num arithmetic/compare, Text `+`/comparison) keep their
        // inline lowering below; they are not entered in `self.overloads`.
        let sym = operator.symbol();
        if self.overloads.contains_key(sym) {
            let arg_types = [self.infer_type(left), self.infer_type(right)];
            if let Some(symbol) = self.resolve_overload_symbol(sym, &arg_types) {
                let l = self.generate_expression(left)?;
                let r = self.generate_expression(right)?;
                return self.build_direct_call(&symbol, &[l, r]);
            }
        }

        // `+` on arrays is concatenation / append / prepend — all produce a NEW array.
        // Arrays and Text both lower to `{ptr,size}` structs, so distinguish by the
        // oracle's Quilon type and route BEFORE the generic StructValue path below (which
        // is Text concat). Triggered when either operand is an array: `[]T + []T`,
        // `[]T + T` (append), or `T + []T` (prepend).
        if operator == BinaryOperator::Add
            && (matches!(self.oracle.expression_type(left), Some(Type::Array(_)))
                || matches!(self.oracle.expression_type(right), Some(Type::Array(_))))
        {
            return self.generate_array_concat(left, right);
        }

        // Set algebra: `+` union, `-` difference, `+-`/`-+` intersection — each builds a
        // NEW set. Distinguished from numeric `+`/`-` by the oracle type; `SetIntersect`
        // (`+-`/`-+`) is only ever a set operator. Routed BEFORE eager operand evaluation
        // so a set operand isn't mistaken for a Num.
        if operator == BinaryOperator::SetIntersect
            || (matches!(operator, BinaryOperator::Add | BinaryOperator::Sub)
                && (matches!(self.oracle.expression_type(left), Some(Type::Set(_)))
                    || matches!(self.oracle.expression_type(right), Some(Type::Set(_)))))
        {
            return self.generate_set_op(operator, left, right);
        }

        // `&&`/`||` are SHORT-CIRCUIT (docs/expressions/README.md "Logical: `&& || !` (short-circuit)"):
        // the right operand must NOT be evaluated when the left already decides the
        // result — `i < a.size && a[i] == k` must never index out of bounds, and a
        // side-effecting right operand must not run. Lower with control flow BEFORE the
        // eager operand evaluation below.
        if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
            return self.generate_short_circuit(operator, left, right);
        }

        // The built-in `Text` operators: `+` concatenates, and `==`/`!=`/`<`/`<=`/`>`/`>=`
        // go through the `__text_cmp` runtime intrinsic (which returns -1/0/1, compared
        // against 0 with the matching integer predicate). Routed on BOTH operands' Quilon
        // type, never on their LLVM shape — arrays, closures and sums are `{ .. }` structs
        // too, and reading one of those as a `{ ptr, len }` Text would be a type confusion.
        // Everything below this point is numeric or Bool, and Text never reaches it.
        if matches!(
            operator,
            BinaryOperator::Add
                | BinaryOperator::Eq
                | BinaryOperator::Ne
                | BinaryOperator::Lt
                | BinaryOperator::Le
                | BinaryOperator::Gt
                | BinaryOperator::Ge
        ) && self.is_text_expression(left)
            && self.is_text_expression(right)
        {
            let lhs = self.generate_expression(left)?;
            let rhs = self.generate_expression(right)?;
            if operator != BinaryOperator::Add {
                return self.generate_text_compare(operator, lhs, rhs);
            }
            let (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) = (lhs, rhs)
            else {
                return Err("Text concatenation requires two Text values".to_string());
            };
            return self.generate_text_concat(l, r);
        }

        let lhs = self.generate_expression(left)?;
        let rhs = self.generate_expression(right)?;

        match operator {
            BinaryOperator::Add => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_add(l, r, "addtmp")
                    .map_err(ctx("Failed to build add"))?
                    .into()),
                _ => Err("Add requires two Nums or two Texts".to_string()),
            },
            BinaryOperator::Sub => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_sub(l, r, "subtmp")
                        .map_err(ctx("Failed to build sub"))?
                        .into())
                } else {
                    Err("Sub operation requires float values".to_string())
                }
            }
            BinaryOperator::Mul => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_mul(l, r, "multmp")
                        .map_err(ctx("Failed to build mul"))?
                        .into())
                } else {
                    Err("Mul operation requires float values".to_string())
                }
            }
            BinaryOperator::Div => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_div(l, r, "divtmp")
                        .map_err(ctx("Failed to build div"))?
                        .into())
                } else {
                    Err("Div operation requires float values".to_string())
                }
            }
            BinaryOperator::Mod => {
                // f64 remainder (LLVM `frem` == C `fmod`): the result takes the
                // DIVIDEND's sign — `7 % 3` is 1, `-7 % 3` is -1, `7 % -3` is 1.
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_rem(l, r, "modtmp")
                        .map_err(ctx("Failed to build mod"))?
                        .into())
                } else {
                    Err("Mod operation requires float values".to_string())
                }
            }
            BinaryOperator::Eq => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l, r, "eqtmp")
                    .map_err(ctx("Failed to build compare"))?
                    .into()),
                // Bool == Bool (both i1) compares the integer values.
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, l, r, "eqtmp")
                    .map_err(ctx("Failed to build compare"))?
                    .into()),
                _ => Err("Eq requires two Nums or two Bools".to_string()),
            },
            BinaryOperator::Ne => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l, r, "netmp")
                    .map_err(ctx("Failed to build compare"))?
                    .into()),
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, l, r, "netmp")
                    .map_err(ctx("Failed to build compare"))?
                    .into()),
                _ => Err("Ne requires two Nums or two Bools".to_string()),
            },
            BinaryOperator::Lt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLT, l, r, "lttmp")
                        .map_err(ctx("Failed to build compare"))?
                        .into())
                } else {
                    Err("Lt operation requires float values".to_string())
                }
            }
            BinaryOperator::Le => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLE, l, r, "letmp")
                        .map_err(ctx("Failed to build compare"))?
                        .into())
                } else {
                    Err("Le operation requires float values".to_string())
                }
            }
            BinaryOperator::Gt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGT, l, r, "gttmp")
                        .map_err(ctx("Failed to build compare"))?
                        .into())
                } else {
                    Err("Gt operation requires float values".to_string())
                }
            }
            BinaryOperator::Ge => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGE, l, r, "getmp")
                        .map_err(ctx("Failed to build compare"))?
                        .into())
                } else {
                    Err("Ge operation requires float values".to_string())
                }
            }
            // `&&`/`||` never reach here — `generate_binary_operator` routes them to
            // `generate_short_circuit` before operand evaluation.
            _ => Err(format!("Unsupported binary operation: {:?}", operator)),
        }
    }

    /// Lower `&&`/`||` with SHORT-CIRCUIT control flow: evaluate the left operand, and
    /// only branch into the right operand when the left does not already decide the
    /// result (`false` decides `&&`; `true` decides `||`). The merged value is a phi of
    /// the deciding constant and the right operand's boolean. Shape mirrors
    /// `generate_if`.
    pub(super) fn generate_short_circuit(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Logical operator outside of function".to_string())?;

        let lhs_val = self.generate_expression(left)?;
        let lhs_bool = self.value_to_boolean(lhs_val)?;
        // The left operand may itself have emitted branches; the phi's incoming edge is
        // the block we END in, not the one we started in.
        let lhs_end = self
            .builder
            .get_insert_block()
            .ok_or("Logical operator outside of a block")?;

        let rhs_bb = self.context.append_basic_block(function, "sc_rhs");
        let merge_bb = self.context.append_basic_block(function, "sc_merge");

        // `&&`: a true left falls through to the right, a false left decides.
        // `||`: a false left falls through to the right, a true left decides.
        let (true_bb, false_bb) = match operator {
            BinaryOperator::And => (rhs_bb, merge_bb),
            _ => (merge_bb, rhs_bb),
        };
        self.builder
            .build_conditional_branch(lhs_bool, true_bb, false_bb)
            .map_err(ctx("Failed to build branch"))?;

        self.builder.position_at_end(rhs_bb);
        let rhs_val = self.generate_expression(right)?;
        let rhs_bool = self.value_to_boolean(rhs_val)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to build branch"))?;
        let rhs_end = self
            .builder
            .get_insert_block()
            .ok_or("Logical operator outside of a block")?;

        self.builder.position_at_end(merge_bb);
        // On the skipped-right edge the left operand already decided the result, so the
        // value it carries is exactly `lhs_bool` (`&&` took this edge only when it was
        // false, `||` only when it was true). No separate deciding constant is needed.
        let phi = self
            .builder
            .build_phi(self.context.bool_type(), "sctmp")
            .map_err(ctx("Failed to build phi"))?;
        phi.add_incoming(&[(&lhs_bool, lhs_end), (&rhs_bool, rhs_end)]);
        Ok(phi.as_basic_value())
    }

    // Helper to convert a value to boolean (i1)
    pub(super) fn value_to_boolean(
        &mut self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) => {
                // Already an int - check if it's i1
                if i.get_type().get_bit_width() == 1 {
                    Ok(i)
                } else {
                    // Convert to i1 by comparing with 0
                    self.builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            i,
                            i.get_type().const_zero(),
                            "tobool",
                        )
                        .map_err(ctx("Failed to convert to bool"))
                }
            }
            BasicValueEnum::FloatValue(f) => {
                // Convert float to bool by comparing with 0.0
                self.builder
                    .build_float_compare(
                        inkwell::FloatPredicate::ONE, // Ordered Not Equal
                        f,
                        f.get_type().const_zero(),
                        "tobool",
                    )
                    .map_err(ctx("Failed to convert float to bool"))
            }
            _ => Err("Cannot convert value to boolean".to_string()),
        }
    }

    pub(super) fn generate_unary_operator(
        &mut self,
        operator: UnaryOperator,
        expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let val = self.generate_expression(expression)?;

        match operator {
            UnaryOperator::Neg => {
                if let BasicValueEnum::FloatValue(f) = val {
                    Ok(self
                        .builder
                        .build_float_neg(f, "negtmp")
                        .map_err(ctx("Failed to build neg"))?
                        .into())
                } else {
                    Err("Neg operation requires float value".to_string())
                }
            }
            UnaryOperator::Not => {
                if let BasicValueEnum::IntValue(i) = val {
                    Ok(self
                        .builder
                        .build_not(i, "nottmp")
                        .map_err(ctx("Failed to build not"))?
                        .into())
                } else {
                    Err("Not operation requires int value".to_string())
                }
            }
        }
    }

    pub(super) fn generate_if(
        &mut self,
        cond: &Expression,
        then_expression: &Expression,
        else_expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let cond_val = self.generate_expression(cond)?;

        let cond_bool = if let BasicValueEnum::IntValue(i) = cond_val {
            i
        } else {
            return Err("Condition must be a boolean".to_string());
        };

        let function = self
            .current_function
            .ok_or_else(|| "If expression outside of function".to_string())?;

        // Create blocks
        let then_bb = self.context.append_basic_block(function, "then");
        let else_bb = self.context.append_basic_block(function, "else");
        let merge_bb = self.context.append_basic_block(function, "ifcont");

        // Build conditional branch
        self.builder
            .build_conditional_branch(cond_bool, then_bb, else_bb)
            .map_err(ctx("Failed to build conditional branch"))?;

        // Generate then block
        self.builder.position_at_end(then_bb);
        let then_val = self.generate_expression(then_expression)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to build branch"))?;
        let then_bb = self.builder.get_insert_block().unwrap();

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_val = self.generate_expression(else_expression)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to build branch"))?;
        let else_bb = self.builder.get_insert_block().unwrap();

        // Generate merge block
        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(then_val.get_type(), "iftmp")
            .map_err(ctx("Failed to build phi"))?;
        phi.add_incoming(&[(&then_val, then_bb), (&else_val, else_bb)]);

        Ok(phi.as_basic_value())
    }

    pub(super) fn generate_block(
        &mut self,
        statements: &[crate::ast::Statement],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Under `--debug`, a `{ }` block introduces a nested lexical scope so its locals nest
        // under a `DW_TAG_lexical_block` rather than the function directly (a no-op otherwise).
        let saved_scope = self.begin_di_lexical_block(span);
        let mut result = self.context.f64_type().const_float(0.0).into();

        for statement in statements {
            match statement {
                crate::ast::Statement::Item(item) => {
                    self.generate_item(item)?;
                }
                crate::ast::Statement::Expression(expression) => {
                    result = self.generate_expression(expression)?;
                }
            }
        }

        self.end_di_scope(saved_scope);
        Ok(result)
    }

    /// Lower an array index `array[index]`. `index_node` is the whole `Expression::Index`
    /// (used to look up the element type in the oracle — the checker records an index
    /// expression's type as its element type); `array` and `index_expression` are its parts.
    /// Only arrays are indexable; the checker rejects `map[key]` (maps are read via
    /// `.get`), so a map value never reaches here.
    pub(super) fn generate_index(
        &mut self,
        index_node: &Expression,
        array: &Expression,
        index_expression: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Generate the array expression
        let array_val = self.generate_expression(array)?;

        // Generate the index expression
        let index_val = self.generate_expression(index_expression)?;

        // An array is a `{ ptr data, i64 size }` struct. To index it: read the data ptr and
        // size fields, bounds-check the index, convert it f64->i64, then GEP + load the elem.
        if let BasicValueEnum::StructValue(_) = array_val {
            // Read the `{ptr, size}` fields straight out of the SSA struct value with
            // `extractvalue` — no stack `alloca`/store round-trip. This is load-bearing for
            // the constant-stack tail-call guarantee: `generate_index` emits at the current
            // insert point, so any `alloca` here would land INSIDE a lowered tail-recursion
            // loop and re-allocate on every iteration, growing the stack without bound until
            // it overflows. Extraction keeps the field reads purely in registers.
            let data_ptr = self.array_data_field(array_val)?;
            let size = self.array_size_field(array_val)?;

            let idx_f = if let BasicValueEnum::FloatValue(f) = index_val {
                f
            } else {
                return Err("Index must be a number".to_string());
            };

            // CHECKED indexing (fail loud, never silent): an out-of-bounds, negative,
            // or NaN index is a clear runtime error (stderr + exit 1), never a raw read.
            // The check runs on the f64 BEFORE `fptosi` — converting an invalid index
            // is poison. A fractional in-range index truncates toward zero (documented).
            let in_bounds = self.index_in_bounds(idx_f, size)?;
            let function = self
                .current_function
                .ok_or_else(|| "Index expression outside of function".to_string())?;
            let fail_bb = self.context.append_basic_block(function, "idx_fail");
            let ok_bb = self.context.append_basic_block(function, "idx_ok");
            self.builder
                .build_conditional_branch(in_bounds, ok_bb, fail_bb)
                .map_err(ctx("Failed to branch on index bounds"))?;

            self.builder.position_at_end(fail_bb);
            let fail_fn = self.get_intrinsic("__index_fail")?;
            // Where the program wrote `arr[i]`, so the report points at the read rather
            // than leaving the reader to guess which one of them it was.
            let site = self.site_value(index_node.span())?;
            self.builder
                .build_call(fail_fn, &[idx_f.into(), size.into(), site.into()], "")
                .map_err(ctx("Failed to call __index_fail"))?;
            self.builder
                .build_unreachable()
                .map_err(ctx("Failed to build unreachable"))?;

            self.builder.position_at_end(ok_bb);
            let index_i64 = self
                .builder
                .build_float_to_signed_int(idx_f, self.context.i64_type(), "index_i64")
                .map_err(ctx("Failed to convert index"))?;

            // Element LLVM type comes from the type oracle (the index expression's type
            // IS the element type), NOT from a hardcoded `f64` — so `Text`/array/record
            // elements load correctly. The element memory was laid out by `generate_array`
            // using this same value representation.
            let elem_llvm = self.oracle_value_type(index_node)?;

            // Use GEP (indexing by element type) to get the element pointer, then load it.
            let elem_ptr = unsafe {
                self.builder
                    .build_gep(elem_llvm, data_ptr, &[index_i64], "elem_ptr")
                    .map_err(ctx("Failed to build GEP"))?
            };

            self.builder
                .build_load(elem_llvm, elem_ptr, "elem")
                .map_err(ctx("Failed to load element"))
        } else {
            Err("Can only index into arrays".to_string())
        }
    }
}
