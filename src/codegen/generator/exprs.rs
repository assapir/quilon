//! The expression dispatcher and the general-purpose expression forms: operators,
//! conditionals, blocks, and indexing.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower `expr` to a value, then force it in place if the deferred-taint pass marked this
    /// span a force site (a deferred `Text` sitting where a strict primitive reads its bytes).
    /// The force is the ONE seam where deferral becomes visible to codegen; everywhere else a
    /// deferred value is threaded as an ordinary `Text`. For a non-force-site span (every
    /// expression in a pure program) this is a direct call — byte-identical to before.
    pub(super) fn generate_expr(&mut self, expr: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        let value = self.generate_expr_inner(expr)?;
        if self.defer.is_force_site(expr.span()) {
            return self.force_deferred_text(value);
        }
        Ok(value)
    }

    fn generate_expr_inner(&mut self, expr: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        // Attribute the instructions this expression lowers to its source location, so the
        // DWARF line table maps generated code back to the `.ql` line (no-op without debug).
        self.set_debug_loc(expr.span());
        match expr {
            Expr::Number { value, .. } => {
                // For now, use f64 for all numbers
                Ok(self.context.f64_type().const_float(*value).into())
            }

            Expr::String { value, .. } => {
                // Text is { ptr data, i64 byte_len }. `data` points at a global,
                // NUL-terminated C string (so `print` can treat it as a C string);
                // `byte_len` is the UTF-8 byte length, excluding the terminator.
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

            Expr::Interpolation { parts, .. } => self.generate_interpolation(parts),

            Expr::Bool { value, .. } => Ok(self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into()),

            // The unit value `$`: a zero `i8` placeholder. The value is never
            // inspected; it just needs a concrete, single-inhabitant representation.
            Expr::Unit { .. } => Ok(self.unit_value().into()),

            Expr::Ident { name, .. } => {
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

            Expr::BinOp {
                left, op, right, ..
            } => self.generate_binop(left, *op, right),

            Expr::UnaryOp { op, expr, .. } => self.generate_unary_op(*op, expr),

            Expr::Call { func, args, span } => self.generate_call(func, args, span),

            Expr::Lambda {
                params,
                return_type,
                body,
                ..
            } => self.generate_lambda(params, return_type.as_ref(), body),

            Expr::If {
                cond, then, else_, ..
            } => self.generate_if(cond, then, else_),

            Expr::Block { stmts, span } => self.generate_block(stmts, span),

            Expr::Array { elements, .. } => self.generate_array(expr, elements),

            Expr::Record { fields, .. } => self.generate_record_expr(expr, fields),

            // A bare spread never survives to codegen on its own — the parser only
            // produces one as an element of an array literal or a field of a record
            // literal, where `generate_array` / `generate_record_expr` consume it.
            Expr::Spread { .. } => {
                Err("spread `<-` is only valid inside an array or record literal".to_string())
            }

            Expr::Constructor {
                type_name, fields, ..
            } => {
                // A `<-source` entry fills the fields it is not overriding, exactly as in
                // an anonymous literal — and the update lowering already builds its result
                // from the whole literal's oracle type, which here is this named type, so
                // the slots land in declaration order with the type's methods intact.
                if fields.iter().any(|(_, v)| matches!(v, Expr::Spread { .. })) {
                    return self.generate_record_update(expr, fields);
                }
                // A named-type instance has the same struct representation as a record,
                // but its field SLOTS follow the type's DECLARATION order — which is the
                // order `record_types` and the type oracle use to index/GEP fields later.
                // The constructor call may list fields in any order, so reorder them to
                // declaration order before lowering; otherwise a later `obj.field` read
                // would GEP the wrong slot (silent corruption once fields differ in type).
                let ordered = self.constructor_fields_in_decl_order(type_name, fields);
                self.generate_record(&ordered)
            }

            Expr::FieldAccess { expr, field, .. } => self.generate_field_access(expr, field),

            Expr::FieldAssign { target, value, .. } => self.generate_field_assign(target, value),

            Expr::Index {
                expr: array, index, ..
            } => self.generate_index(expr, array, index),

            Expr::Match {
                expr: scrutinee,
                arms,
                ..
            } => self.generate_match(expr, scrutinee, arms),

            Expr::Range { start, end, .. } => self.generate_range(start, end),

            // `left |> right` desugars to a call with `left` as the first arg
            // (must match the type checker's desugaring exactly).
            Expr::Pipeline { left, right, span } => {
                let call = Expr::desugar_pipeline(left, right, span);
                self.generate_expr(&call)
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

        let ready_block = self.builder.get_insert_block().unwrap();
        let function = ready_block.get_parent().unwrap();
        let force_block = self.context.append_basic_block(function, "force");
        let cont_block = self.context.append_basic_block(function, "force_cont");
        self.builder
            .build_conditional_branch(is_deferred, force_block, cont_block)
            .map_err(ctx("Failed to branch on deferred sentinel"))?;

        // Deferred: extract the promise pointer and force it (park-until-ready, memoized).
        self.builder.position_at_end(force_block);
        let promise = self
            .builder
            .build_extract_value(deferred, 0, "deferred_promise")
            .map_err(ctx("Failed to read deferred promise"))?
            .into_pointer_value();
        let force_fn = self.get_intrinsic("__force_text")?;
        let forced = Self::call_result_to_basic(
            self.builder
                .build_call(force_fn, &[promise.into()], "forced")
                .map_err(ctx("Failed to call __force_text"))?,
        )?;
        let force_end_block = self.builder.get_insert_block().unwrap();
        self.builder
            .build_unconditional_branch(cont_block)
            .map_err(ctx("Failed to branch out of force"))?;

        // Join: the ready value (unchanged) or the forced value.
        self.builder.position_at_end(cont_block);
        let phi = self
            .builder
            .build_phi(text_ty, "forced_or_ready")
            .map_err(ctx("Failed to build force phi"))?;
        phi.add_incoming(&[(&value, ready_block), (&forced, force_end_block)]);
        Ok(phi.as_basic_value())
    }

    pub(super) fn generate_binop(
        &mut self,
        left: &Expr,
        op: BinOp,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A USER operator overload (e.g. `+`/`==` on a record type) lowers to a direct
        // call to its mangled function — the operator is just a named overload set.
        // Built-in operators (Num arithmetic/compare, Text `+`/comparison) keep their
        // inline lowering below; they are not entered in `self.overloads`.
        let sym = op.symbol();
        if self.overloads.contains_key(sym) {
            let arg_types = [self.infer_type(left), self.infer_type(right)];
            if let Some(symbol) = self.resolve_overload_symbol(sym, &arg_types) {
                let l = self.generate_expr(left)?;
                let r = self.generate_expr(right)?;
                return self.build_direct_call(&symbol, &[l, r]);
            }
        }

        // `+` on arrays is concatenation / append / prepend — all produce a NEW array.
        // Arrays and Text both lower to `{ptr,size}` structs, so distinguish by the
        // oracle's Quilon type and route BEFORE the generic StructValue path below (which
        // is Text concat). Triggered when either operand is an array: `[]T + []T`,
        // `[]T + T` (append), or `T + []T` (prepend).
        if op == BinOp::Add
            && (matches!(self.oracle.expr_type(left), Some(Type::Array(_)))
                || matches!(self.oracle.expr_type(right), Some(Type::Array(_))))
        {
            return self.generate_array_concat(left, right);
        }

        // `&&`/`||` are SHORT-CIRCUIT (docs/LANGUAGE.md "Logical: `&& || !` (short-circuit)"):
        // the right operand must NOT be evaluated when the left already decides the
        // result — `i < a.size && a[i] == k` must never index out of bounds, and a
        // side-effecting right operand must not run. Lower with control flow BEFORE the
        // eager operand evaluation below.
        if matches!(op, BinOp::And | BinOp::Or) {
            return self.generate_short_circuit(op, left, right);
        }

        let lhs = self.generate_expr(left)?;
        let rhs = self.generate_expr(right)?;

        // Text comparison: both operands are `Text` { ptr, i64 } structs. Lower
        // equality and lexicographic ordering via the `__text_cmp` runtime intrinsic
        // (returns -1/0/1), then compare its result against 0 with the matching
        // integer predicate. (Num operands fall through to the float paths below.)
        if matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        ) && matches!(lhs, BasicValueEnum::StructValue(_))
            && matches!(rhs, BasicValueEnum::StructValue(_))
        {
            return self.generate_text_compare(op, lhs, rhs);
        }

        match op {
            BinOp::Add => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_add(l, r, "addtmp")
                    .map_err(ctx("Failed to build add"))?
                    .into()),
                // Text + Text = concatenation (both are { ptr, i64 } structs).
                (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) => {
                    self.generate_text_concat(l, r)
                }
                _ => Err("Add requires two Nums or two Texts".to_string()),
            },
            BinOp::Sub => {
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
            BinOp::Mul => {
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
            BinOp::Div => {
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
            BinOp::Mod => {
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
            BinOp::Eq => match (lhs, rhs) {
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
            BinOp::Ne => match (lhs, rhs) {
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
            BinOp::Lt => {
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
            BinOp::Le => {
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
            BinOp::Gt => {
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
            BinOp::Ge => {
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
            // `&&`/`||` never reach here — `generate_binop` routes them to
            // `generate_short_circuit` before operand evaluation.
            _ => Err(format!("Unsupported binary operation: {:?}", op)),
        }
    }

    /// Lower `&&`/`||` with SHORT-CIRCUIT control flow: evaluate the left operand, and
    /// only branch into the right operand when the left does not already decide the
    /// result (`false` decides `&&`; `true` decides `||`). The merged value is a phi of
    /// the deciding constant and the right operand's boolean. Shape mirrors
    /// `generate_if`.
    pub(super) fn generate_short_circuit(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Logical operator outside of function".to_string())?;

        let lhs_val = self.generate_expr(left)?;
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
        let (true_bb, false_bb) = match op {
            BinOp::And => (rhs_bb, merge_bb),
            _ => (merge_bb, rhs_bb),
        };
        self.builder
            .build_conditional_branch(lhs_bool, true_bb, false_bb)
            .map_err(ctx("Failed to build branch"))?;

        self.builder.position_at_end(rhs_bb);
        let rhs_val = self.generate_expr(right)?;
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

    pub(super) fn generate_unary_op(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let val = self.generate_expr(expr)?;

        match op {
            UnaryOp::Neg => {
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
            UnaryOp::Not => {
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
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let cond_val = self.generate_expr(cond)?;

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
        let then_val = self.generate_expr(then_expr)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(ctx("Failed to build branch"))?;
        let then_bb = self.builder.get_insert_block().unwrap();

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_val = self.generate_expr(else_expr)?;
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
        stmts: &[crate::ast::Statement],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Under `--debug`, a `{ }` block introduces a nested lexical scope so its locals nest
        // under a `DW_TAG_lexical_block` rather than the function directly (a no-op otherwise).
        let saved_scope = self.begin_di_lexical_block(span);
        let mut result = self.context.f64_type().const_float(0.0).into();

        for stmt in stmts {
            match stmt {
                crate::ast::Statement::Item(item) => {
                    self.generate_item(item)?;
                }
                crate::ast::Statement::Expr(expr) => {
                    result = self.generate_expr(expr)?;
                }
            }
        }

        self.end_di_scope(saved_scope);
        Ok(result)
    }

    /// Lower an array index `array[index]`. `index_node` is the whole `Expr::Index`
    /// (used to look up the element type in the oracle — the checker records an index
    /// expression's type as its element type); `array` and `index_expr` are its parts.
    pub(super) fn generate_index(
        &mut self,
        index_node: &Expr,
        array: &Expr,
        index_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Generate the array expression
        let array_val = self.generate_expr(array)?;

        // Generate the index expression
        let index_val = self.generate_expr(index_expr)?;

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
            self.builder
                .build_call(fail_fn, &[idx_f.into(), size.into()], "")
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
