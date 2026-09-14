//! The provided assertions: lowering `assert(actual, matcher)` and `expect(actual, matcher)`.
//!
//! A matcher is not a value — the compiler provides the whole form (see
//! [`crate::ast::MATCHERS`]), which is what lets one matcher name work over every type
//! without generics. Each lowers to the condition it tests plus the description of what it
//! wanted, and the description is only rendered on the failing path.
//!
//! `assert` reports and exits; `expect` reports, marks the running case failed, and ends the
//! case — `generate_run_case` is what makes that possible. `expect` also reads the case's
//! failed mark before doing anything else, a defensive fallback for whatever reaches it with
//! no case actually running to end (see the comment at that check, below).
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

        // Defensive: a failing `expect` (below) ends the case by suspending its fiber, so a
        // later `expect` in the same case is never reached to ask this at all. Kept as the
        // fallback for wherever that does not apply — it still does nothing at all, not
        // even evaluating the value under test, so a failure never reports twice.
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

        let actual_type = self.oracle_type(actual, "the value under test")?;
        let actual_value = self.generate_expression(actual)?;
        let failed = self.context.append_basic_block(function, "assert_failed");

        // `aborts()` (however many `not`s wrap it) runs the value under test rather than
        // comparing it, and its failure message shows what actually happened (the withheld
        // report, or that the lambda returned) instead of the generic "expected …, got …" —
        // its own lowering, kept apart from the generic matcher pipeline below.
        let message = match Self::aborts_nesting(matcher) {
            Some(nesting) => {
                let (held, did_abort) =
                    self.generate_aborts_held(&actual_type, actual_value, nesting)?;
                self.builder
                    .build_conditional_branch(held, done, failed)
                    .map_err(ctx("Failed to branch on an assertion"))?;
                self.builder.position_at_end(failed);
                self.build_aborts_message(did_abort)?
            }
            None => {
                let mut wanted = Vec::new();
                let held =
                    self.matcher_condition(&actual_type, actual_value, matcher, &mut wanted)?;
                self.builder
                    .build_conditional_branch(held, done, failed)
                    .map_err(ctx("Failed to branch on an assertion"))?;
                self.builder.position_at_end(failed);
                let mut pieces = vec![Piece::Literal("assertion failed: expected ".to_string())];
                pieces.append(&mut wanted);
                pieces.push(Piece::Literal(", got ".to_string()));
                pieces.push(Piece::Value(actual_type, actual_value));
                self.build_message(pieces)?
            }
        };
        self.report_assertion_failure(fatal, message, span)?;
        // `__assert_failed` never returns, but the branch keeps the block terminated the
        // ordinary way — as `__exit` does — so the assertion composes wherever an
        // expression is expected.
        self.builder
            .build_unconditional_branch(done)
            .map_err(ctx("Failed to leave an assertion's failure path"))?;

        self.builder.position_at_end(done);
        Ok(self.unit_value().into())
    }

    /// Report a failing assertion at `span` through `__assert_failed`/`__expect_failed`, as
    /// `fatal` selects — the shared tail of every lowering `generate_assertion` picks
    /// between, once `message` (a `Text`) is built.
    fn report_assertion_failure(
        &mut self,
        fatal: bool,
        message: BasicValueEnum<'ctx>,
        span: &Span,
    ) -> Result<(), String> {
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
        Ok(())
    }

    /// How many `not(...)` layers separate `matcher` from an `aborts()` leaf, or `None` if
    /// it is not (possibly negated) `aborts()` at all.
    fn aborts_nesting(matcher: &Expression) -> Option<u32> {
        let Expression::Call {
            function,
            arguments,
            ..
        } = matcher
        else {
            return None;
        };
        let Expression::Identifier { name, .. } = function.as_ref() else {
            return None;
        };
        match name.as_str() {
            "aborts" => Some(0),
            "not" => Self::aborts_nesting(arguments.first()?).map(|nesting| nesting + 1),
            _ => None,
        }
    }

    /// Run the lambda under test on a guarded fiber (`__abort_trap_run`) and yield the
    /// assertion's condition (the trap outcome XORed by `nesting`'s parity) alongside the
    /// RAW outcome — `nesting` decides whether the assertion passes, but the failure
    /// message (built afterward, lazily) always phrases itself around what actually
    /// happened, regardless of how many `not`s asked for the opposite.
    fn generate_aborts_held(
        &mut self,
        actual_type: &Type,
        actual: BasicValueEnum<'ctx>,
        nesting: u32,
    ) -> Result<
        (
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        String,
    > {
        let Type::Function { return_type, .. } = actual_type else {
            return Err("`aborts()` reads a zero-parameter lambda".to_string());
        };
        let closure = actual.into_struct_value();
        let bundle = self.bundle_closure(closure, "aborts_bundle")?;

        let thunk = self.emit_abort_trap_thunk(return_type)?;
        let thunk_ptr = thunk.as_global_value().as_pointer_value();

        let run = self.get_intrinsic("__abort_trap_run")?;
        let call = self
            .builder
            .build_call(run, &[thunk_ptr.into(), bundle.into()], "aborts_run")
            .map_err(ctx("Failed to call the abort trap"))?;
        let result = Self::call_result_to_basic(call)?.into_int_value();
        let did_abort = self
            .int_to_bool(result, "aborts_did_abort")?
            .into_int_value();

        let held = match nesting % 2 {
            0 => did_abort,
            _ => self
                .builder
                .build_not(did_abort, "aborts_not")
                .map_err(ctx("Failed to negate an aborts condition"))?,
        };
        Ok((held, did_abort))
    }

    /// A top-level trampoline `(ptr bundle) -> i8` that unpacks the `{ ptr fn, ptr env }`
    /// bundle `generate_aborts_held` built and calls `fn(env)` with `return_type`'s own
    /// calling convention, discarding the result. `__abort_trap_run` calls it in place of
    /// the lambda's actual function pointer, whose signature varies with `return_type` and
    /// so cannot be called from the runtime directly — this is what lets one Rust-side
    /// entry point run a lambda of any return type.
    fn emit_abort_trap_thunk(&mut self, return_type: &Type) -> Result<FunctionValue<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let ret_ty = self.boundary_type(return_type)?;

        let fn_type = self.context.i8_type().fn_type(&[ptr_ty.into()], false);
        let name = format!("__aborts_thunk_{}", self.lambda_counter);
        self.lambda_counter += 1;
        let function = self.module.add_function(&name, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);

        let suspended = self.suspend_enclosing_function();
        self.current_function = Some(function);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let bundle = function.get_nth_param(0).unwrap().into_pointer_value();
        let (real_fn, real_env) = self.unpack_closure_bundle(bundle, "aborts_bundle")?;

        let call_type = ret_ty.fn_type(&[ptr_ty.into()], false);
        self.builder
            .build_indirect_call(call_type, real_fn, &[real_env.into()], "aborts_body_call")
            .map_err(ctx("Failed to call the aborts lambda"))?;
        self.builder
            .build_return(Some(&self.context.i8_type().const_zero()))
            .map_err(ctx("Failed to return from the aborts trampoline"))?;

        self.resume_enclosing_function(suspended);
        Ok(function)
    }

    /// The failure message for `aborts()` (or `not(aborts())`): what actually happened —
    /// the withheld report if the lambda aborted, or that it returned if it did not — is
    /// not known until `did_abort` is read at run time, so this branches in IR rather than
    /// choosing a literal at compile time.
    fn build_aborts_message(
        &mut self,
        did_abort: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "aborts() message outside a function".to_string())?;
        let aborted_block = self
            .context
            .append_basic_block(function, "aborts_msg_aborted");
        let returned_block = self
            .context
            .append_basic_block(function, "aborts_msg_returned");
        let merge_block = self
            .context
            .append_basic_block(function, "aborts_msg_merge");
        self.builder
            .build_conditional_branch(did_abort, aborted_block, returned_block)
            .map_err(ctx("Failed to branch on the abort trap's outcome"))?;

        self.builder.position_at_end(aborted_block);
        let report = self.get_intrinsic("__abort_trap_report")?;
        let report_call = self
            .builder
            .build_call(report, &[], "aborts_report")
            .map_err(ctx("Failed to fetch the withheld abort report"))?;
        let report_text = Self::call_result_to_basic(report_call)?;
        let prefix = self
            .text_literal("assertion failed: expected the lambda not to abort, but it aborted: ")?;
        let aborted_message =
            self.generate_text_concat(prefix.into_struct_value(), report_text.into_struct_value())?;
        self.builder
            .build_unconditional_branch(merge_block)
            .map_err(ctx("Failed to leave the aborted-message branch"))?;
        let aborted_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(returned_block);
        let returned_message =
            self.text_literal("assertion failed: expected the lambda to abort, but it returned")?;
        self.builder
            .build_unconditional_branch(merge_block)
            .map_err(ctx("Failed to leave the returned-message branch"))?;
        let returned_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(merge_block);
        let phi = self
            .builder
            .build_phi(self.ptr_len_struct_type(), "aborts_message")
            .map_err(ctx("Failed to merge the aborts() message"))?;
        phi.add_incoming(&[
            (&aborted_message, aborted_end),
            (&returned_message, returned_end),
        ]);
        Ok(phi.as_basic_value())
    }

    /// Lower `__test_run_case(body)` (see [`crate::ast::RUN_TEST_CASE`]): split `body`'s
    /// `{ ptr fn, ptr env }` value apart and hand both to `__test_case_run_guarded`, which
    /// runs `fn(env)` on its own nested fiber — a failing `expect` ends the case by
    /// suspending that fiber, however deeply nested inside the call tree it is reached.
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
        let run = self.get_intrinsic("__test_case_run_guarded")?;
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
                let expected_type =
                    self.oracle_type(&arguments[0], "an `equals` matcher's expected value")?;
                let expected = self.generate_expression(&arguments[0])?;
                wanted.push(Piece::Value(expected_type, expected));
                self.values_equal(actual_type, actual, expected)
            }
            "contains" => {
                let part_type =
                    self.oracle_type(&arguments[0], "a `contains` matcher's expected part")?;
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
