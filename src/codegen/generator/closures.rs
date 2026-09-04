//! Lambdas and closures: deciding what a nested function captures and how (by value or
//! through a box), emitting its body, and calling one.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Bind a capturing nested function as a local closure value: lower it via the lambda
    /// machinery (capturing enclosing locals per the `=`/`:=` rule) and store the
    /// resulting `{ ptr fn, ptr env }` in a local slot. `name(args)` then resolves to an
    /// indirect closure call from the checker's recorded `Type::Function` for that
    /// identifier — nothing further is recorded here.
    pub(super) fn generate_local_closure(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), String> {
        let closure = self.generate_lambda(
            &declaration.parameters,
            declaration.declared_return_type(),
            &declaration.body,
        )?;
        let slot = self.create_entry_block_alloca(&declaration.name, closure.get_type())?;
        self.builder
            .build_store(slot, closure)
            .map_err(ctx("Failed to store closure"))?;
        self.variables
            .insert(declaration.name.clone(), (slot, closure.get_type()));
        Ok(())
    }

    pub(super) fn create_entry_block_alloca(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let builder = self.context.create_builder();

        let entry = self
            .current_function
            .unwrap()
            .get_first_basic_block()
            .unwrap();
        match entry.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry),
        }

        builder
            .build_alloca(ty, name)
            .map_err(ctx("Failed to build alloca"))
    }

    // ---- Closures (M3) -----------------------------------------------------------------
    //
    // A closure value is a flat `{ ptr fn, ptr env }` struct: a pointer to the lifted
    // top-level function, and a pointer to its heap-allocated environment of captured
    // values. The lifted function takes the captured environment as an extra TRAILING
    // pointer parameter (after the source parameters), so calling through a closure is a
    // plain indirect call passing `env` last. Closures are monomorphic (M3): captured
    // values and parameters are concrete-typed; generic closures are M4.

    /// The uniform closure representation: `{ ptr fn, ptr env }`.
    pub(super) fn closure_struct_type(&self) -> inkwell::types::StructType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.context.struct_type(&[ptr.into(), ptr.into()], false)
    }

    /// Allocate a GC-managed heap cell large enough to hold one `ty` value and return the
    /// pointer to it. Used to "box" a `:=` local captured by reference, so the cell
    /// outlives the defining frame and is shared with the closure.
    pub(super) fn alloc_box(&self, ty: BasicTypeEnum<'ctx>) -> Result<PointerValue<'ctx>, String> {
        use inkwell::values::AnyValue;
        let size = ty
            .size_of()
            .ok_or_else(|| format!("cannot size box for type {:?}", ty))?;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        Ok(self
            .builder
            .build_call(alloc_fn, &[size.into()], "box")
            .map_err(ctx("Failed to call __alloc for box"))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// The `:=` (mutable) locals of the function body `body` that some nested closure
    /// captures by reference, and so must be heap-boxed. A captured `=` local is copied
    /// by value into the closure's environment and needs no box; only a captured mutable
    /// local must share a single cell with the closure. Computed by collecting the
    /// function's `:=` binding names and intersecting with the union of every nested
    /// lambda's free variables.
    pub(super) fn compute_boxed_vars(
        &self,
        body: &Expression,
    ) -> std::collections::HashSet<String> {
        let mut mutable_locals = std::collections::HashSet::new();
        Self::collect_mutable_locals(body, &mut mutable_locals);

        // Find which of those mutable locals a nested closure captures. Passing the
        // mutable-local set as the lambdas' `outer` scope means a closure's captures are
        // already restricted to (and recognize reassignments of) exactly these names.
        let mut captured = std::collections::HashSet::new();
        Self::collect_lambda_captures(body, &mutable_locals, &mut captured);
        captured
    }

    /// Collect the names of all `:=` (mutable) `VariableDeclaration`s bound in THIS function frame —
    /// i.e. in `expression` and its nested control-flow, but NOT inside a nested lambda body (a
    /// lambda's own `:=` locals live in the lambda's frame, not ours).
    pub(super) fn collect_mutable_locals(
        expression: &Expression,
        out: &mut std::collections::HashSet<String>,
    ) {
        match expression {
            // A nested function literal opens its own frame — do not descend.
            Expression::Lambda { .. } => {}
            Expression::Block { statements, .. } => {
                for statement in statements {
                    match statement {
                        crate::ast::Statement::Expression(e) => {
                            Self::collect_mutable_locals(e, out)
                        }
                        crate::ast::Statement::Item(Item::VariableDeclaration(declaration)) => {
                            if declaration.mutable {
                                out.insert(declaration.name.clone());
                            }
                            Self::collect_mutable_locals(&declaration.value, out);
                        }
                        crate::ast::Statement::Item(_) => {}
                    }
                }
            }
            Expression::BinaryOperator { left, right, .. }
            | Expression::Range {
                start: left,
                end: right,
                ..
            } => {
                Self::collect_mutable_locals(left, out);
                Self::collect_mutable_locals(right, out);
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::FieldAccess { expression, .. } => {
                Self::collect_mutable_locals(expression, out)
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                Self::collect_mutable_locals(function, out);
                for a in arguments {
                    Self::collect_mutable_locals(a, out);
                }
            }
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                Self::collect_mutable_locals(condition, out);
                Self::collect_mutable_locals(then, out);
                Self::collect_mutable_locals(else_, out);
            }
            Expression::Match {
                expression, arms, ..
            } => {
                Self::collect_mutable_locals(expression, out);
                for arm in arms {
                    Self::collect_mutable_locals(&arm.body, out);
                }
            }
            Expression::FieldAssign { target, value, .. } => {
                Self::collect_mutable_locals(target, out);
                Self::collect_mutable_locals(value, out);
            }
            Expression::Index {
                expression, index, ..
            } => {
                Self::collect_mutable_locals(expression, out);
                Self::collect_mutable_locals(index, out);
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for e in elements {
                    Self::collect_mutable_locals(e, out);
                }
            }
            Expression::MapLiteral { entries, .. } => {
                for (k, v) in entries {
                    Self::collect_mutable_locals(k, out);
                    Self::collect_mutable_locals(v, out);
                }
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, e) in fields {
                    Self::collect_mutable_locals(e, out);
                }
            }
            Expression::Spread { expression, .. } => Self::collect_mutable_locals(expression, out),
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let crate::ast::InterpolationPart::Hole(e) = part {
                        Self::collect_mutable_locals(e, out);
                    }
                }
            }
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. }
            | Expression::Identifier { .. } => {}
        }
    }

    /// Union, over every lambda appearing (at any depth) in `expression`, of the names it
    /// captures from `outer`. Used to find which of the enclosing frame's mutable locals
    /// a closure shares (and so must be heap-boxed).
    pub(super) fn collect_lambda_captures(
        expression: &Expression,
        outer: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        Self::for_each_closure(expression, &mut |parameters, body| {
            let names: Vec<String> = parameters.iter().map(|p| p.name.clone()).collect();
            for name in crate::ast::captures::lambda_free_idents(&names, body, outer) {
                out.insert(name);
            }
        });
    }

    /// Invoke `f(parameters, body)` for every closure appearing (at any depth) in `expression` — a
    /// `Expression::Lambda` OR a nested `Item::FunctionDeclaration` (both are closures; the latter is
    /// only resolved to a plain function at codegen when it captures nothing). Used to
    /// gather captures across all closures in a frame.
    pub(super) fn for_each_closure(
        expression: &Expression,
        f: &mut impl FnMut(&[crate::ast::Parameter], &Expression),
    ) {
        Self::walk_expressions(expression, &mut |e| match e {
            Expression::Lambda {
                parameters, body, ..
            } => f(parameters, body),
            Expression::Block { statements, .. } => {
                for statement in statements {
                    if let crate::ast::Statement::Item(Item::FunctionDeclaration(declaration)) =
                        statement
                    {
                        f(&declaration.parameters, &declaration.body);
                        // The function body is an expression position `walk_expressions` does
                        // not enter (it only descends VariableDeclaration initializers), so recurse to
                        // find closures nested inside this nested function too.
                        Self::for_each_closure(&declaration.body, f);
                    }
                }
            }
            _ => {}
        });
    }

    /// Pre-order walk over every sub-expression of `expression`, invoking `f` on each. Used by
    /// the closure pre-passes above. Does not descend into nested item declarations'
    /// signatures (only expression positions), which is all closure analysis needs.
    pub(super) fn walk_expressions(expression: &Expression, f: &mut impl FnMut(&Expression)) {
        f(expression);
        match expression {
            Expression::BinaryOperator { left, right, .. }
            | Expression::Range {
                start: left,
                end: right,
                ..
            } => {
                Self::walk_expressions(left, f);
                Self::walk_expressions(right, f);
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::FieldAccess { expression, .. } => Self::walk_expressions(expression, f),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                Self::walk_expressions(function, f);
                for a in arguments {
                    Self::walk_expressions(a, f);
                }
            }
            Expression::Lambda { body, .. } => Self::walk_expressions(body, f),
            Expression::Block { statements, .. } => {
                for statement in statements {
                    match statement {
                        crate::ast::Statement::Expression(e) => Self::walk_expressions(e, f),
                        crate::ast::Statement::Item(Item::VariableDeclaration(d)) => {
                            Self::walk_expressions(&d.value, f)
                        }
                        crate::ast::Statement::Item(_) => {}
                    }
                }
            }
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                Self::walk_expressions(condition, f);
                Self::walk_expressions(then, f);
                Self::walk_expressions(else_, f);
            }
            Expression::Match {
                expression, arms, ..
            } => {
                Self::walk_expressions(expression, f);
                for arm in arms {
                    Self::walk_expressions(&arm.body, f);
                }
            }
            Expression::FieldAssign { target, value, .. } => {
                Self::walk_expressions(target, f);
                Self::walk_expressions(value, f);
            }
            Expression::Index {
                expression, index, ..
            } => {
                Self::walk_expressions(expression, f);
                Self::walk_expressions(index, f);
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for e in elements {
                    Self::walk_expressions(e, f);
                }
            }
            Expression::MapLiteral { entries, .. } => {
                for (k, v) in entries {
                    Self::walk_expressions(k, f);
                    Self::walk_expressions(v, f);
                }
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, e) in fields {
                    Self::walk_expressions(e, f);
                }
            }
            Expression::Spread { expression, .. } => Self::walk_expressions(expression, f),
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let crate::ast::InterpolationPart::Hole(e) = part {
                        Self::walk_expressions(e, f);
                    }
                }
            }
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. }
            | Expression::Identifier { .. } => {}
        }
    }

    /// The expression a body evaluates to: a block's is its tail statement, so that is the
    /// one carrying the body's type.
    fn body_value(body: &Expression) -> &Expression {
        match body {
            Expression::Block { statements, .. } => match statements.last() {
                Some(crate::ast::Statement::Expression(tail)) => Self::body_value(tail),
                _ => body,
            },
            _ => body,
        }
    }

    /// The default return TYPE codegen assigns a function with the given (possibly
    /// missing) return annotation and body: the annotation if present, else `$` (Unit)
    /// for a Unit-tailed body, else `Num`. Codegen lacks the checker's full inference, so
    /// this picks the LLVM-level return type for an unannotated function/lambda/method.
    /// (The entry point `^` is handled separately — it always returns an f64 exit code.)
    pub(super) fn default_return_type(
        &self,
        return_type: Option<&Type>,
        body: &Expression,
    ) -> Type {
        // The oracle records types by span, and a block has none of its own — so ask about
        // the expression the body evaluates to. Every function body is a block, so without
        // this an unannotated `make = () => < Ok("hello") >` lowers its return to the `Num`
        // fallback and the payload a downstream match binds is the wrong shape.
        let body = Self::body_value(body);
        match return_type {
            // A GENERIC annotation — only `-> Result`, whose `Ok(T)`/`NotOk(E)` payload
            // slots are type variables the language can't otherwise name — is refined to
            // the body's concrete type from the oracle, so the LLVM return matches the
            // value the body actually produces (`Ok("x")` => `{ i8, Text }`, not the
            // generic `{ i8, double }`). Mirrors the checker refining a generic return.
            Some(t) if t.contains_generic() => self
                .oracle
                .expression_type(body)
                .cloned()
                .unwrap_or_else(|| t.clone()),
            // A concrete annotation is authoritative.
            Some(t) => t.clone(),
            None if self.expression_is_unit(body) => Type::Unit,
            // Unannotated: the oracle holds the checker's inferred body type (concrete,
            // including a specialized Result payload such as `Result[Ok(Text)]`), so a
            // function returning `Ok("x")` lowers its return to the payload's real shape
            // and a downstream match binds it usably. Fall back to `Num` for the IR-only
            // codegen tests that run without a type-check pass.
            None => self
                .oracle
                .expression_type(body)
                .cloned()
                .unwrap_or(Type::Num),
        }
    }

    /// The LLVM signature of a function literal: (source-parameter types, return type).
    /// Mirrors the type rules used when emitting the lifted function, but without the
    /// trailing env pointer (which is implicit to every closure call).
    pub(super) fn closure_signature(
        &self,
        parameters: &[crate::ast::Parameter],
        return_type: Option<&Type>,
        body: &Expression,
    ) -> Result<ClosureSig<'ctx>, String> {
        let parameter_types: Vec<BasicTypeEnum> = parameters
            .iter()
            .map(|p| self.boundary_type(&self.parameter_type(p)))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = self.default_return_type(return_type, body);
        Ok((parameter_types, self.boundary_type(&ret)?))
    }

    /// Lower a function literal to a value: lift its body into a fresh top-level function
    /// taking the captured environment as a trailing `ptr` parameter, build and populate
    /// that environment on the heap, and return the `{ ptr fn, ptr env }` closure struct.
    ///
    /// Capture rule, inferred from each captured name's binding operator:
    ///   `=`  binding -> captured BY VALUE: a snapshot is copied into the env (read-only).
    ///   `:=` binding -> captured BY REFERENCE: the env holds the pointer to the shared
    ///                   GC cell (the box), so reads see — and writes escape to — the one
    ///                   cell, surviving the closure outliving its defining frame.
    pub(super) fn generate_lambda(
        &mut self,
        parameters: &[crate::ast::Parameter],
        return_type: Option<&Type>,
        body: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        // 1. Determine the captured names: a lambda free variable that is actually a
        //    binding in the current frame. A by-reference capture is one whose name is in
        //    the current `boxed_vars` (its storage is a shared cell); the rest are by-value.
        let parameter_names: Vec<String> = parameters.iter().map(|p| p.name.clone()).collect();
        let outer: std::collections::HashSet<String> = self.variables.keys().cloned().collect();
        let free = crate::ast::captures::lambda_free_idents(&parameter_names, body, &outer);
        let mut captures: Vec<Capture<'ctx>> = Vec::new();
        for name in free {
            // Only names with a live local slot are captured; a free name that resolves to
            // a top-level function/global is referenced directly inside the lifted body
            // (it has module scope) and needs no capture.
            if let Some((slot, value_ty)) = self.variables.get(&name).copied() {
                let by_ref = self.boxed_vars.contains(&name);
                captures.push(Capture {
                    name,
                    slot,
                    value_ty,
                    by_ref,
                });
            }
        }

        // 2. Build the environment struct type. A by-value capture stores the value; a
        //    by-reference capture stores the cell pointer (`ptr`).
        let env_field_types: Vec<BasicTypeEnum> = captures
            .iter()
            .map(|c| if c.by_ref { ptr_ty.into() } else { c.value_ty })
            .collect();
        let env_struct_ty = self.context.struct_type(&env_field_types, false);

        // 3. Allocate and populate the environment on the GC heap (so it survives the
        //    closure escaping). For a by-value capture, snapshot the current value; for a
        //    by-reference capture, store the shared cell pointer itself.
        let env_ptr = if captures.is_empty() {
            ptr_ty.const_null()
        } else {
            let env = self.alloc_box(env_struct_ty.into())?;
            for (i, cap) in captures.iter().enumerate() {
                let field = self
                    .builder
                    .build_struct_gep(env_struct_ty, env, i as u32, "env_field")
                    .map_err(ctx("Failed to GEP env field"))?;
                let stored: BasicValueEnum = if cap.by_ref {
                    cap.slot.into()
                } else {
                    self.builder
                        .build_load(cap.value_ty, cap.slot, &cap.name)
                        .map_err(ctx("Failed to load capture"))?
                };
                self.builder
                    .build_store(field, stored)
                    .map_err(ctx("Failed to store capture"))?;
            }
            env
        };

        // 4. Emit the lifted top-level function `__lambda_N(parameters..., ptr env)`.
        let fn_value =
            self.emit_lambda_function(parameters, return_type, body, &captures, env_struct_ty)?;

        // 5. Assemble the closure value `{ fn_ptr, env_ptr }`.
        let closure_ty = self.closure_struct_type();
        let fn_ptr = fn_value.as_global_value().as_pointer_value();
        let with_fn = self
            .builder
            .build_insert_value(closure_ty.get_undef(), fn_ptr, 0, "clo_fn")
            .map_err(ctx("Failed to insert closure fn"))?
            .into_struct_value();
        let closure = self
            .builder
            .build_insert_value(with_fn, env_ptr, 1, "clo_env")
            .map_err(ctx("Failed to insert closure env"))?
            .into_struct_value();
        Ok(closure.into())
    }

    /// Emit the lifted top-level function for a lambda: its source parameters followed by
    /// a trailing `ptr env`. Inside, parameters are bound normally and each captured name
    /// is re-bound from the environment — a by-value capture is copied into a local slot,
    /// a by-reference capture re-uses the shared cell pointer directly (so writes escape).
    /// Saves and restores the enclosing codegen state (current function, variable scope,
    /// boxed set, builder position) around the nested emission.
    pub(super) fn emit_lambda_function(
        &mut self,
        parameters: &[crate::ast::Parameter],
        return_type: Option<&Type>,
        body: &Expression,
        captures: &[Capture<'ctx>],
        env_struct_ty: inkwell::types::StructType<'ctx>,
    ) -> Result<FunctionValue<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        // Same (parameter types, return type) the call site reconstructs, plus the trailing
        // env pointer — keeping the emitted function and the indirect-call type in lockstep.
        let (mut parameter_types, ret_ty) =
            self.closure_signature(parameters, return_type, body)?;
        parameter_types.push(ptr_ty.into()); // trailing env pointer

        let fn_type = ret_ty.fn_type(
            &parameter_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        let name = format!("__lambda_{}", self.lambda_counter);
        self.lambda_counter += 1;
        let function = self.module.add_function(&name, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);

        // Save enclosing emission state — we are about to emit a DIFFERENT function body.
        let saved_block = self.builder.get_insert_block();
        let saved_function = self.current_function;
        let saved_frame = self.take_frame();
        // The lifted body has its own frame: recompute which of ITS `:=` locals are boxed.
        self.boxed_vars = self.compute_boxed_vars(body);
        // A by-reference capture is ALSO a shared cell in this frame: mark it boxed so a
        // FURTHER nested closure capturing the same name captures it by reference too
        // (sharing the one cell across all nesting levels). Without this, a `:=` value
        // mutated through two levels of closures would be silently snapshotted by value.
        for cap in captures.iter().filter(|c| c.by_ref) {
            self.boxed_vars.insert(cap.name.clone());
        }
        // A captured variable's Quilon-type metadata must travel into the lifted frame
        // with it: field access, method dispatch, and overloaded-call mangling on the
        // captured name inside the closure body all resolve through these maps.
        for cap in captures {
            if let Some(qty) = saved_frame.var_types.get(&cap.name) {
                self.var_types.insert(cap.name.clone(), qty.clone());
            }
            if let Some(fields) = saved_frame.record_types.get(&cap.name) {
                self.record_types.insert(cap.name.clone(), fields.clone());
            }
            if let Some(type_name) = saved_frame.var_named_types.get(&cap.name) {
                self.var_named_types
                    .insert(cap.name.clone(), type_name.clone());
            }
        }
        self.current_function = Some(function);
        let saved_scope = self.begin_di_function(function, &name, body.span());

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Bind source parameters (indices 0..n); the env pointer is the last parameter.
        for (i, parameter) in parameters.iter().enumerate() {
            let llvm_parameter = function.get_nth_param(i as u32).unwrap();
            llvm_parameter.set_name(&parameter.name);
            let pty = llvm_parameter.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&parameter.name, pty)?;
            self.builder
                .build_store(alloca, llvm_parameter)
                .map_err(ctx("Failed to store parameter"))?;
            self.variables.insert(parameter.name.clone(), (alloca, pty));
            let qty = self.parameter_type(parameter);
            self.declare_variable(
                &parameter.name,
                alloca,
                &qty,
                &parameter.span,
                Some((i + 1) as u32),
            );
        }

        // Re-bind captures from the environment pointer (the trailing parameter).
        if !captures.is_empty() {
            let env_ptr = function
                .get_nth_param(parameters.len() as u32)
                .unwrap()
                .into_pointer_value();
            for (i, cap) in captures.iter().enumerate() {
                let field = self
                    .builder
                    .build_struct_gep(env_struct_ty, env_ptr, i as u32, "cap_field")
                    .map_err(ctx("Failed to GEP capture field"))?;
                if cap.by_ref {
                    // The field holds the shared cell pointer; load it and bind the name
                    // to that cell so reads/writes inside the closure hit the one cell.
                    let cell = self
                        .builder
                        .build_load(ptr_ty, field, &cap.name)
                        .map_err(ctx("Failed to load cell ptr"))?
                        .into_pointer_value();
                    self.variables
                        .insert(cap.name.clone(), (cell, cap.value_ty));
                    self.declare_variable(
                        &cap.name,
                        cell,
                        self.var_types.get(&cap.name).unwrap_or(&Type::Num),
                        body.span(),
                        None,
                    );
                } else {
                    // By-value capture: copy the snapshot into a fresh local slot.
                    let val = self
                        .builder
                        .build_load(cap.value_ty, field, &cap.name)
                        .map_err(ctx("Failed to load capture value"))?;
                    let alloca = self.create_entry_block_alloca(&cap.name, cap.value_ty)?;
                    self.builder
                        .build_store(alloca, val)
                        .map_err(ctx("Failed to store capture value"))?;
                    self.variables
                        .insert(cap.name.clone(), (alloca, cap.value_ty));
                    self.declare_variable(
                        &cap.name,
                        alloca,
                        self.var_types.get(&cap.name).unwrap_or(&Type::Num),
                        body.span(),
                        None,
                    );
                }
            }
        }

        let body_value = self.generate_expression(body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(ctx("Failed to build closure return"))?;

        // Restore the enclosing emission state.
        self.restore_frame(saved_frame);
        self.current_function = saved_function;
        self.end_di_scope(saved_scope);
        if let Some(block) = saved_block {
            self.builder.position_at_end(block);
            // Reactivate the enclosing function's source scope for whatever it emits next
            // (the closure value assembly), now that this nested body is closed out.
            if self.di_scope.is_some() {
                self.set_debug_loc(body.span());
            }
        }

        Ok(function)
    }

    /// Call a function-valued EXPRESSION — a local variable holding a closure, a call on
    /// a call (`adder(5)(2)`), or any other callee that is not a plain top-level function
    /// name: generate it to a closure `{ ptr fn, ptr env }` value and call through it,
    /// recovering the callee signature from the oracle's recorded type for the expression.
    pub(super) fn generate_closure_value_call(
        &mut self,
        function: &Expression,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Some(Type::Function {
            parameters,
            return_type,
        }) = self.oracle.expression_type(function).cloned()
        else {
            return Err(
                "the callee is not a function name or a function-valued expression".to_string(),
            );
        };
        if args.len() != parameters.len() {
            return Err(format!(
                "this closure expects {} argument(s), got {}",
                parameters.len(),
                args.len()
            ));
        }
        let parameter_tys = parameters
            .iter()
            .map(|t| self.boundary_type(t))
            .collect::<Result<Vec<_>, _>>()?;
        let ret_ty = self.boundary_type(&return_type)?;
        let closure_val = self.generate_expression(function)?.into_struct_value();
        self.call_closure_value(closure_val, &parameter_tys, ret_ty, args)
    }

    /// The shared tail of every closure call: split the `{ ptr fn, ptr env }` value into
    /// its function and environment pointers and emit an indirect call passing the source
    /// arguments followed by the environment pointer.
    fn call_closure_value(
        &mut self,
        closure_val: inkwell::values::StructValue<'ctx>,
        parameter_tys: &[BasicTypeEnum<'ctx>],
        ret_ty: BasicTypeEnum<'ctx>,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_ptr = self
            .builder
            .build_extract_value(closure_val, 0, "clo_fn")
            .map_err(ctx("Failed to extract closure fn"))?
            .into_pointer_value();
        let env_ptr = self
            .builder
            .build_extract_value(closure_val, 1, "clo_env")
            .map_err(ctx("Failed to extract closure env"))?
            .into_pointer_value();

        // Evaluate arguments, then append the environment pointer.
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
            Vec::with_capacity(args.len() + 1);
        for arg in args {
            call_args.push(self.generate_expression(arg)?.into());
        }
        call_args.push(env_ptr.into());

        // Reconstruct the callee function type: source parameters + trailing env ptr -> ret.
        let mut metadata_parameters: Vec<inkwell::types::BasicMetadataTypeEnum> =
            parameter_tys.iter().map(|t| (*t).into()).collect();
        metadata_parameters.push(ptr_ty.into());
        let fn_type = ret_ty.fn_type(&metadata_parameters, false);

        let call = self
            .builder
            .build_indirect_call(fn_type, fn_ptr, &call_args, "clo_call")
            .map_err(ctx("Failed to build indirect call"))?;

        Self::call_result_to_basic(call)
    }
}
