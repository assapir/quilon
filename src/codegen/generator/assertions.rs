//! The provided assertions: lowering `assert(actual, matcher)` and `expect(actual, matcher)`.
//!
//! A matcher is not a value — the compiler provides the whole form (see
//! [`crate::ast::MATCHERS`]), which is what lets one matcher name work over every type
//! without generics. Each lowers to the condition it tests plus the description of what it
//! wanted, and the description is only rendered on the failing path.
//!
//! `assert` reports and exits; `expect` reports, marks the running case failed, and returns —
//! and reads that mark first, so a case's later assertions are skipped once one has failed.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these methods
//! run against.

use super::*;

/// One piece of a failure message: literal text, or a value rendered through its `` ` ``.
enum Piece<'ctx> {
    Literal(String),
    Value(Type, BasicValueEnum<'ctx>),
}

impl<'ctx> CodeGenerator<'ctx> {
    /// Lower `assert`/`expect`. `span` is the call's own location, which the failure is
    /// framed around. Yields `$` (Unit), so an assertion composes in expression position.
    pub(super) fn generate_assertion(
        &mut self,
        name: &str,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let [actual, matcher] = arguments else {
            return Err(format!(
                "{name} takes the value under test and a matcher, got {} argument(s)",
                arguments.len()
            ));
        };
        let fatal = name == crate::ast::ASSERT;
        let function = self
            .current_function
            .ok_or_else(|| format!("{name} outside a function"))?;
        let done = self.context.append_basic_block(function, "assert_done");

        // An `expect` in a case that has already failed does nothing at all — not even
        // evaluating the value under test. That is what makes a failure skip the rest of
        // its case while the suite carries on.
        if !fatal {
            let live = self.context.append_basic_block(function, "expect_live");
            let failing = self.generate_test_registry("__test_case_failing", &[])?;
            let unfailed = self
                .builder
                .build_float_compare(
                    inkwell::FloatPredicate::OEQ,
                    failing.into_float_value(),
                    self.context.f64_type().const_zero(),
                    "case_unfailed",
                )
                .map_err(ctx("Failed to compare the case's failed mark"))?;
            self.builder
                .build_conditional_branch(unfailed, live, done)
                .map_err(ctx("Failed to branch on the case's failed mark"))?;
            self.builder.position_at_end(live);
        }

        let actual_type = self.infer_type(actual);
        let actual_value = self.generate_expression(actual)?;
        let mut wanted = Vec::new();
        let held = self.matcher_condition(&actual_type, actual_value, matcher, &mut wanted)?;

        let failed = self.context.append_basic_block(function, "assert_failed");
        self.builder
            .build_conditional_branch(held, done, failed)
            .map_err(ctx("Failed to branch on an assertion"))?;

        self.builder.position_at_end(failed);
        let mut pieces = vec![Piece::Literal("assertion failed: expected ".to_string())];
        pieces.append(&mut wanted);
        pieces.push(Piece::Literal(", got ".to_string()));
        pieces.push(Piece::Value(actual_type, actual_value));
        let message = self.build_message(pieces)?;
        let (message_ptr, message_len) = self.text_fields(message)?;
        let site = self.site_value(span)?;
        let report = self.get_intrinsic(match fatal {
            true => "__assert_failed",
            false => "__expect_failed",
        })?;
        self.builder
            .build_call(
                report,
                &[site.into(), message_ptr.into(), message_len.into()],
                "",
            )
            .map_err(ctx("Failed to build an assertion report call"))?;
        // `__assert_failed` never returns, but the branch keeps the block terminated the
        // ordinary way — as `__exit` does — so the assertion composes wherever an
        // expression is expected.
        self.builder
            .build_unconditional_branch(done)
            .map_err(ctx("Failed to leave an assertion's failure path"))?;

        self.builder.position_at_end(done);
        Ok(self.unit_value().into())
    }

    /// Lower `__test_run_case(body)` (see [`crate::ast::RUN_TEST_CASE`]): split `body`'s
    /// `{ ptr fn, ptr env }` value apart and hand both to `__test_case_run`, which calls
    /// `fn(env)` with a bail-out point recorded first — a failing `expect` ends the case by
    /// jumping straight back to it (`quilon-rt/src/case_guard.c`), skipping whatever of the
    /// body is left however deeply the failure is nested. `runCase` calls this instead of
    /// calling `body` directly, which is what a flag checked between statements cannot do
    /// once the body has called into a lambda of its own (`.each`, and the like).
    pub(super) fn generate_run_case(
        &mut self,
        arguments: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let [body] = arguments else {
            return Err(format!(
                "__test_run_case takes exactly the case body, got {} argument(s)",
                arguments.len()
            ));
        };
        let closure = self.generate_expression(body)?.into_struct_value();
        let function_pointer = self
            .builder
            .build_extract_value(closure, 0, "case_body_fn")
            .map_err(ctx("Failed to extract the case body's function pointer"))?;
        let environment = self
            .builder
            .build_extract_value(closure, 1, "case_body_env")
            .map_err(ctx("Failed to extract the case body's environment"))?;
        let run = self.get_intrinsic("__test_case_run")?;
        self.builder
            .build_call(run, &[function_pointer.into(), environment.into()], "")
            .map_err(ctx("Failed to call the guarded case runner"))?;
        Ok(self.unit_value().into())
    }

    /// Lower one matcher against the already-evaluated value under test: yields the `i1`
    /// condition that HOLDS when the assertion passes, and appends to `wanted` the
    /// description of what it wanted (rendered only if the assertion fails).
    fn matcher_condition(
        &mut self,
        actual_type: &Type,
        actual: BasicValueEnum<'ctx>,
        matcher: &Expression,
        wanted: &mut Vec<Piece<'ctx>>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let Expression::Call {
            function,
            arguments,
            ..
        } = matcher
        else {
            return Err("an assertion's second argument must be a matcher".to_string());
        };
        let Expression::Identifier { name, .. } = function.as_ref() else {
            return Err("an assertion's second argument must be a matcher".to_string());
        };
        match name.as_str() {
            "equals" => {
                let expected_type = self.infer_type(&arguments[0]);
                let expected = self.generate_expression(&arguments[0])?;
                wanted.push(Piece::Value(expected_type, expected));
                self.values_equal(actual_type, actual, expected)
            }
            "contains" => {
                let part_type = self.infer_type(&arguments[0]);
                let part = self.generate_expression(&arguments[0])?;
                wanted.push(Piece::Literal("something containing ".to_string()));
                wanted.push(Piece::Value(part_type, part));
                match actual_type {
                    Type::Text => self.text_contains(actual, part),
                    Type::Array(element) => {
                        let element = (**element).clone();
                        self.array_contains(&element, actual, part)
                    }
                    other => Err(format!(
                        "`contains` reads a Text or an array, not {}",
                        crate::ast::type_label(other)
                    )),
                }
            }
            "not" => {
                wanted.push(Piece::Literal("not ".to_string()));
                let held = self.matcher_condition(actual_type, actual, &arguments[0], wanted)?;
                self.builder
                    .build_not(held, "matcher_not")
                    .map_err(ctx("Failed to negate a matcher"))
            }
            variant_matcher => {
                let variant = crate::ast::matcher_variant(variant_matcher)
                    .ok_or_else(|| format!("`{variant_matcher}` is not a matcher"))?;
                wanted.push(Piece::Literal(variant.to_string()));
                self.variant_tag_matches(variant, actual)
            }
        }
    }

    /// `left == right` at the value level, over the same members `==` itself dispatches to:
    /// a type's own `==` first, then the built-in scalar comparisons. The checker has
    /// already refused a type with neither.
    fn values_equal(
        &mut self,
        ty: &Type,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        if let Some(symbol) = self.resolve_overload_symbol("==", &[ty.clone(), ty.clone()]) {
            let equal = self.build_direct_call(&symbol, &[left, right])?;
            return Ok(equal.into_int_value());
        }
        match ty {
            Type::Text => Ok(self
                .generate_text_compare(BinaryOperator::Eq, left, right)?
                .into_int_value()),
            // A not-yet-concrete sum payload (`Generic`) is represented as a Num.
            Type::Num | Type::Generic { .. } => self
                .builder
                .build_float_compare(
                    inkwell::FloatPredicate::OEQ,
                    left.into_float_value(),
                    right.into_float_value(),
                    "matcher_eq",
                )
                .map_err(ctx("Failed to compare two Nums")),
            Type::Bool => self
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    left.into_int_value(),
                    right.into_int_value(),
                    "matcher_eq",
                )
                .map_err(ctx("Failed to compare two Bools")),
            other => Err(format!(
                "no `==` member to compare {} with",
                crate::ast::type_label(other)
            )),
        }
    }

    /// Whether the `Text` `haystack` contains `part`, via the same intrinsic `.contains()`
    /// lowers to.
    fn text_contains(
        &mut self,
        haystack: BasicValueEnum<'ctx>,
        part: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let (haystack_ptr, haystack_len) = self.text_fields(haystack)?;
        let (part_ptr, part_len) = self.text_fields(part)?;
        let contains = self.get_intrinsic("__text_contains")?;
        let found = self
            .builder
            .build_call(
                contains,
                &[
                    haystack_ptr.into(),
                    haystack_len.into(),
                    part_ptr.into(),
                    part_len.into(),
                ],
                "text_contains",
            )
            .map_err(ctx("Failed to build a text-contains call"))?;
        let found = Self::call_result_to_basic(found)?.into_int_value();
        Ok(self
            .int_to_bool(found, "text_contains_found")?
            .into_int_value())
    }

    /// Whether the array holds an element equal to `part`. Scans with the element type's own
    /// `==`, which is what makes this work for an array of user records.
    fn array_contains(
        &mut self,
        element_type: &Type,
        array: BasicValueEnum<'ctx>,
        part: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let data = self.array_data_field(array)?;
        let size = self.array_size_field(array)?;
        let element_llvm = self.value_repr_type(element_type)?;
        let bool_type = self.context.bool_type();
        let found = self.create_entry_block_alloca("contains_found", bool_type.into())?;
        self.builder
            .build_store(found, bool_type.const_zero())
            .map_err(ctx("Failed to init the contains flag"))?;
        let element_type = element_type.clone();
        self.array_loop(size, |generator, index| {
            let element = generator.load_element(data, element_llvm, index)?;
            let equal = generator.values_equal(&element_type, element, part)?;
            let seen = generator
                .builder
                .build_load(bool_type, found, "contains_seen")
                .map_err(ctx("Failed to load the contains flag"))?
                .into_int_value();
            let seen = generator
                .builder
                .build_or(seen, equal, "contains_or")
                .map_err(ctx("Failed to update the contains flag"))?;
            generator
                .builder
                .build_store(found, seen)
                .map_err(ctx("Failed to store the contains flag"))?;
            Ok(())
        })?;
        Ok(self
            .builder
            .build_load(bool_type, found, "contains_found_value")
            .map_err(ctx("Failed to load the contains result"))?
            .into_int_value())
    }

    /// Concatenate a failure message's pieces into one `Text`. A value renders through its
    /// `` ` `` — quoted when it is a `Text`, so a trailing space or an empty string shows.
    fn build_message(&mut self, pieces: Vec<Piece<'ctx>>) -> Result<BasicValueEnum<'ctx>, String> {
        let mut message: Option<BasicValueEnum<'ctx>> = None;
        for piece in pieces {
            let rendered = match piece {
                Piece::Literal(text) => self.text_literal(&text)?,
                Piece::Value(Type::Text, value) => {
                    let quote = self.text_literal("\"")?.into_struct_value();
                    let opened = self.generate_text_concat(quote, value.into_struct_value())?;
                    self.generate_text_concat(opened.into_struct_value(), quote)?
                }
                Piece::Value(ty, value) => self.render_value(&ty, value)?,
            };
            message = Some(match message {
                None => rendered,
                Some(so_far) => self.generate_text_concat(
                    so_far.into_struct_value(),
                    rendered.into_struct_value(),
                )?,
            });
        }
        match message {
            Some(message) => Ok(message),
            None => self.text_literal(""),
        }
    }
}
