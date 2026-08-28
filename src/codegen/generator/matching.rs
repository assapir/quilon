//! `?`/`|` pattern matching: testing a scrutinee against each arm's pattern and binding
//! what the pattern names.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower a `match` (`scrutinee ? | pat => body ...`). `match_expression` is the whole
    /// `Expression::Match` node (used only to look up the match's result type in the oracle);
    /// `scrutinee` is the value being matched.
    pub(super) fn generate_match(
        &mut self,
        match_expression: &Expression,
        scrutinee: &Expression,
        arms: &[MatchArm],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Evaluate the expression being matched
        let match_val = self.generate_expression(scrutinee)?;

        // Get the current function
        let function = self
            .current_function
            .ok_or_else(|| "Match expression must be in a function".to_string())?;

        // Create basic blocks for each arm and a continuation block
        let mut arm_blocks = vec![];
        let mut check_blocks = vec![];
        for i in 0..arms.len() {
            check_blocks.push(
                self.context
                    .append_basic_block(function, &format!("check_{}", i)),
            );
            arm_blocks.push(
                self.context
                    .append_basic_block(function, &format!("arm_{}", i)),
            );
        }
        let cont_block = self.context.append_basic_block(function, "match_cont");

        // The result type of the match (the common type of its arm bodies) comes from
        // the type oracle — NOT a hardcoded `f64` — so a match yielding `Text` (e.g. the
        // `Ok(text)` payload) allocates and loads a `Text` struct rather than corrupting
        // it through an f64 slot. Falls back to `f64` if the oracle didn't record it.
        let result_llvm = self.oracle_value_type(match_expression)?;
        let result_alloca = self.create_entry_block_alloca("match_result", result_llvm)?;

        let no_match_block = self.build_no_match_block(match_expression.span())?;

        // Jump to first check
        self.builder
            .build_unconditional_branch(check_blocks[0])
            .map_err(ctx("Failed to build branch"))?;

        // Generate code for each arm
        for (i, arm) in arms.iter().enumerate() {
            // Position at check block
            self.builder.position_at_end(check_blocks[i]);

            // Check if pattern matches
            let matches = self.check_pattern(&arm.pattern, match_val)?;

            // Conditional branch to arm or next check; past the last arm, to the abort.
            let next_block = if i + 1 < check_blocks.len() {
                check_blocks[i + 1]
            } else {
                no_match_block
            };

            self.builder
                .build_conditional_branch(matches, arm_blocks[i], next_block)
                .map_err(ctx("Failed to build conditional branch"))?;

            // Generate arm body
            self.builder.position_at_end(arm_blocks[i]);

            // Bind pattern variables
            self.bind_pattern(&arm.pattern, match_val, scrutinee)?;

            let arm_val = self.generate_expression(&arm.body)?;
            self.builder
                .build_store(result_alloca, arm_val)
                .map_err(ctx("Failed to store result"))?;

            self.builder
                .build_unconditional_branch(cont_block)
                .map_err(ctx("Failed to build branch"))?;
        }

        // Position at continuation block
        self.builder.position_at_end(cont_block);

        // Load the result with the match's declared result type (see `result_llvm`).
        self.builder
            .build_load(result_llvm, result_alloca, "match_result")
            .map_err(ctx("Failed to load result"))
    }

    /// The block a match branches to when no arm matched it: the fail-loud backstop behind
    /// the checker's exhaustiveness rule. It reports at `span` — the match's own location —
    /// and terminates, so the no-match edge can never reach the continuation and load a
    /// result slot no arm ever wrote.
    ///
    /// The block is appended and filled without disturbing the current insert point, so a
    /// caller can build it before wiring its arms.
    pub(super) fn build_no_match_block(
        &mut self,
        span: &Span,
    ) -> Result<inkwell::basic_block::BasicBlock<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Match expression must be in a function".to_string())?;
        let resume = self.builder.get_insert_block();
        let block = self.context.append_basic_block(function, "match_no_arm");

        self.builder.position_at_end(block);
        let fail = self.get_intrinsic("__match_fail")?;
        let site = self.site_value(span)?;
        self.builder
            .build_call(fail, &[site.into()], "")
            .map_err(ctx("Failed to call __match_fail"))?;
        self.builder
            .build_unreachable()
            .map_err(ctx("Failed to build unreachable"))?;

        if let Some(resume) = resume {
            self.builder.position_at_end(resume);
        }
        Ok(block)
    }

    pub(super) fn check_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Wildcard always matches
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Identifier { .. } => {
                // Identifier pattern always matches (binds the value)
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Number { value: num_val, .. } => {
                // Compare the value
                if let BasicValueEnum::FloatValue(fval) = value {
                    let const_val = self.context.f64_type().const_float(*num_val);
                    self.builder
                        .build_float_compare(
                            inkwell::FloatPredicate::OEQ,
                            fval,
                            const_val,
                            "num_match",
                        )
                        .map_err(ctx("Failed to build comparison"))
                } else {
                    Ok(self.context.bool_type().const_zero())
                }
            }

            Pattern::Constructor { name, .. } => match value {
                BasicValueEnum::StructValue(_) => self.variant_tag_matches(name, value),
                // Not a struct - pattern doesn't match
                _ => Ok(self.context.bool_type().const_zero()),
            },
        }
    }

    /// Whether a sum value carries the `variant` variant. A tagged union is
    /// `{ i8 tag, <payload> }`, and the tag is the variant's declaration index, looked up
    /// from the sum-variant registry. Shared by constructor patterns and the `isOk`/`isNotOk`
    /// matchers, so the two can never disagree about a variant's tag.
    pub(super) fn variant_tag_matches(
        &mut self,
        variant: &str,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let expected = self
            .sum_variants
            .get(variant)
            .map(|(tag, _)| *tag)
            .ok_or_else(|| format!("Unknown constructor: {}", variant))?;
        let BasicValueEnum::StructValue(sum) = value else {
            return Err(format!("`{variant}` needs a sum value"));
        };
        let tag = self
            .builder
            .build_extract_value(sum, 0, "tag")
            .map_err(ctx("Failed to extract tag"))?
            .into_int_value();
        self.builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                tag,
                self.context.i8_type().const_int(expected as u64, false),
                "tag_match",
            )
            .map_err(ctx("Failed to compare tags"))
    }

    /// Concrete per-value payload types for the matched constructor `variant`, read from
    /// the SCRUTINEE's oracle type. A scrutinee inferred as `Result[Ok(Text)]` (from
    /// `Ok("x")`) yields `[Text]` for `Ok`, so a payload binding can record its REAL type
    /// for overload mangling — unlike the declared `variant_payloads`, whose `Result`
    /// slots are `Generic` (which would mis-mangle to the `Num` member). `None` when the
    /// oracle has no concrete `Sum` type for the scrutinee.
    pub(super) fn scrutinee_payload_types(
        &self,
        scrutinee: &Expression,
        variant: &str,
    ) -> Option<Vec<Type>> {
        match self.oracle.expression_type(scrutinee)? {
            Type::Sum { variants, .. } => variants
                .iter()
                .find(|v| v.name == variant)
                .map(|v| v.fields.clone()),
            _ => None,
        }
    }

    pub(super) fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
        scrutinee: &Expression,
    ) -> Result<(), String> {
        match pattern {
            Pattern::Identifier { name, .. } => {
                // Bind the value to the identifier
                let alloca = self.create_entry_block_alloca(name, value.get_type())?;
                self.builder
                    .build_store(alloca, value)
                    .map_err(ctx("Failed to store pattern binding"))?;
                self.variables
                    .insert(name.clone(), (alloca, value.get_type()));
                Ok(())
            }

            Pattern::Constructor {
                name, arguments, ..
            } => {
                // Extract each payload field and bind it to the corresponding sub-pattern.
                // The value is `{ i8 tag, payload0, payload1, ... }`, so payload `i` is
                // struct field `i + 1`. Only identifier sub-patterns bind a name; others
                // (wildcards, nested constructors) are matched structurally elsewhere.
                //
                // Each payload binding records its Quilon type in `var_types` (the map
                // that mangles an overloaded call on the binding, e.g.
                // `Ok(s) => describe(s)`), taken from the first NON-generic of two ordered
                // sources:
                //  - the SCRUTINEE's oracle type, whose `Result` payload was specialized
                //    per value (`Ok("x")` => `Result[Ok(Text)]`), so `s` binds as `Text`;
                //  - else the variant's declared payloads (`variant_payloads`), concrete
                //    for a USER sum type (`Circle(Num)`) but `Generic` for `Result`.
                // A still-`Generic` payload is left untracked — an untracked binding
                // defaults to `Num` (the historical behavior), rather than mis-mangling.
                if let BasicValueEnum::StructValue(struct_val) = value {
                    let concrete = self.scrutinee_payload_types(scrutinee, name);
                    let declared = self.variant_payloads.get(name).cloned();
                    // Result stores every payload in one canonical `{ptr,i64}` slot; a bound
                    // payload must be UNPACKED back to its concrete type (from the oracle).
                    let is_result = self
                        .sum_variants
                        .get(name.as_str())
                        .is_some_and(|(_, tn)| tn == "Result");
                    for (i, arg) in arguments.iter().enumerate() {
                        if let Pattern::Identifier { name: arg_name, .. } = arg {
                            let payload_ty = [&concrete, &declared]
                                .into_iter()
                                .filter_map(|src| src.as_ref()?.get(i))
                                .find(|t| !matches!(t, Type::Generic { .. }));
                            let raw = self
                                .builder
                                .build_extract_value(struct_val, (i + 1) as u32, "payload")
                                .map_err(ctx("Failed to extract payload"))?;
                            let payload = if is_result {
                                self.unpack_result_payload(raw, payload_ty)?
                            } else {
                                raw
                            };
                            let alloca =
                                self.create_entry_block_alloca(arg_name, payload.get_type())?;
                            self.builder
                                .build_store(alloca, payload)
                                .map_err(ctx("Failed to store constructor arg"))?;
                            self.variables
                                .insert(arg_name.clone(), (alloca, payload.get_type()));
                            if let Some(ty) = payload_ty {
                                self.var_types.insert(arg_name.clone(), ty.clone());
                                // A named-record payload binds by pointer (the record ABI);
                                // track it so field reads / method calls on the binding resolve.
                                self.track_named_record_binding(arg_name, ty);
                            }
                        }
                    }
                }
                Ok(())
            }

            _ => Ok(()), // Other patterns don't bind variables
        }
    }
}
