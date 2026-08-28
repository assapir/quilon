//! Lowering of top-level and nested declarations: type/record declarations and their
//! methods, `=`/`:=` bindings, and the emission of a function body.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    pub(super) fn generate_type_declaration(
        &mut self,
        declaration: &TypeDeclaration,
    ) -> Result<(), String> {
        let type_name = &declaration.name;

        // Record field names (a sum has none) — used by method bodies for `it.field`.
        let field_names: Vec<String> = match &declaration.type_definition {
            TypeDefinition::Record { fields, .. } => {
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                self.named_type_fields
                    .insert(type_name.clone(), names.clone());
                names
            }
            TypeDefinition::Sum { .. } => Vec::new(),
        };

        let methods = declaration.type_definition.methods();

        // Record which types override the render operator `` ` ``, so a render site
        // dispatches to the override instead of the built-in (type-name/variant) default.
        if methods.iter().any(|m| m.name == "`") {
            self.render_overrides.insert(type_name.clone());
        }

        // Parameter 0 of a receiver-dispatched method is the receiver `it`: a pointer for
        // a record, the value struct for a sum. The shared boundary rule handles both
        // (a bare `Named` name that is a registered sum lowers to the sum struct).
        let receiver_llvm = self.boundary_type(&Type::named_ref(type_name))?;

        // Pass 1: declare every RECEIVER-dispatched method (named methods and the render
        // `` ` ``) first, so a body may call a sibling or recurse regardless of order.
        // Operator members are not methods — they lower to overload functions below.
        for method in methods {
            if crate::ast::is_operator_symbol(&method.name) {
                continue;
            }
            let mangled = method_symbol(type_name, &method.name);
            self.declared_methods
                .entry(method.name.clone())
                .or_default()
                .insert(type_name.to_string());
            if self.module.get_function(&mangled).is_some() {
                continue;
            }
            let mut parameter_types: Vec<inkwell::types::BasicMetadataTypeEnum> =
                vec![receiver_llvm.into()];
            for p in &method.parameters {
                let pt = self.boundary_type(&self.parameter_type(p))?;
                parameter_types.push(pt.into());
            }
            // Unannotated return type defaults to Num, except a setter body whose
            // tail is an in-place field write (`it.field := v`) yields `$` (i8).
            let inferred_ret = self.default_return_type(method.return_type.as_ref(), &method.body);
            let return_type = self.boundary_type(&inferred_ret)?;
            let fn_type = return_type.fn_type(&parameter_types, false);
            let method_fn = self.module.add_function(&mangled, fn_type, None);
            // Internal linkage: method symbols are module-private (see generate_function_declaration).
            method_fn.set_linkage(inkwell::module::Linkage::Internal);
        }

        // Pass 2: generate each receiver-dispatched method body, then lower each operator
        // member to its overload function.
        for method in methods {
            if crate::ast::is_operator_symbol(&method.name) {
                self.emit_operator_member(type_name, method)?;
            } else {
                self.generate_method(type_name, &field_names, method)?;
            }
        }

        // Type declarations are not inside a function; clear any stray function context so a
        // following global declaration is not mistaken for a local.
        self.current_function = None;
        Ok(())
    }

    /// Lower an operator member to its overload function: a function with the receiver `it`
    /// prepended as the first parameter. `emit_module_function` mangles it on the operator
    /// symbol (registered in `self.overloads`), so `a <op> b` dispatches to it.
    fn emit_operator_member(
        &mut self,
        type_name: &str,
        method: &MethodDeclaration,
    ) -> Result<(), String> {
        let mut parameters = Vec::with_capacity(method.parameters.len() + 1);
        parameters.push(crate::ast::Parameter {
            name: crate::ast::RECEIVER.to_string(),
            type_annotation: Some(Type::named_ref(type_name)),
            span: method.span.clone(),
        });
        parameters.extend(method.parameters.iter().cloned());
        let declaration = FunctionDeclaration {
            name: method.name.clone(),
            parameters,
            return_type: method.return_type.clone(),
            binding_type: None,
            body: method.body.clone(),
            exported: false,
            from_corelib: false,
            span: method.span.clone(),
        };
        self.emit_module_function(&declaration)
    }

    /// Emit the body of a single method as the pre-declared `"{TypeName}_{method}"` function,
    /// with `it` bound to the receiver pointer so `it.field` / sibling-method calls resolve.
    pub(super) fn generate_method(
        &mut self,
        type_name: &str,
        field_names: &[String],
        method: &MethodDeclaration,
    ) -> Result<(), String> {
        let mangled = method_symbol(type_name, &method.name);
        let function = self
            .module
            .get_function(&mangled)
            .ok_or_else(|| format!("Method function not declared: {}", mangled))?;
        self.current_function = Some(function);
        // Rendering the receiver `it` wholesale inside the type's own `` ` `` override must
        // use the built-in default, not re-invoke the override — else it recurses forever.
        let prev_backtick = self.generating_backtick_for.take();
        if method.name == "`" {
            self.generating_backtick_for = Some(type_name.to_string());
        }
        let saved_scope = self.begin_di_function(function, &method.name, &method.span);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.take_frame(); // fresh frame: the previously emitted function's entries are dead
        self.boxed_vars = self.compute_boxed_vars(&method.body);

        // Parameter 0 is the implicit receiver `it` (a pointer to the record struct).
        let it_parameter = function.get_nth_param(0).unwrap();
        it_parameter.set_name(crate::ast::RECEIVER);
        let it_type = it_parameter.as_basic_value_enum().get_type();
        let it_alloca = self.create_entry_block_alloca(crate::ast::RECEIVER, it_type)?;
        self.builder
            .build_store(it_alloca, it_parameter)
            .map_err(ctx("Failed to store it"))?;
        self.variables
            .insert(crate::ast::RECEIVER.to_string(), (it_alloca, it_type));
        // So `it.field` and `it.method()` resolve against this type.
        self.record_types
            .insert(crate::ast::RECEIVER.to_string(), field_names.to_vec());
        self.var_named_types
            .insert(crate::ast::RECEIVER.to_string(), type_name.to_string());
        // `it` is the record receiver (parameter #1); build its type only when debug is on.
        if self.debug.is_some() {
            let it_qty = Type::named_ref(type_name);
            self.declare_variable(
                crate::ast::RECEIVER,
                it_alloca,
                &it_qty,
                &method.span,
                Some(1),
            );
        }

        // Remaining parameters follow the receiver.
        for (i, parameter) in method.parameters.iter().enumerate() {
            let llvm_parameter = function.get_nth_param((i + 1) as u32).unwrap();
            llvm_parameter.set_name(&parameter.name);
            let parameter_type = llvm_parameter.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&parameter.name, parameter_type)?;
            self.builder
                .build_store(alloca, llvm_parameter)
                .map_err(ctx("Failed to build store"))?;
            self.variables
                .insert(parameter.name.clone(), (alloca, parameter_type));
            let qty = self.parameter_type(parameter);
            self.register_function_typed_parameter(&parameter.name, &qty)?;
            self.declare_variable(
                &parameter.name,
                alloca,
                &qty,
                &parameter.span,
                Some((i + 2) as u32),
            );
        }

        let body_value = self.generate_expression(&method.body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(ctx("Failed to build return"))?;

        self.generating_backtick_for = prev_backtick;
        self.end_di_scope(saved_scope);
        Ok(())
    }

    pub(super) fn generate_variable_declaration(
        &mut self,
        declaration: &VariableDeclaration,
    ) -> Result<(), String> {
        // Check if this is a record literal to track field names. Prefer the oracle's
        // inferred type (authoritative field names/order, and it expands `<-` spreads);
        // a functional-update whose result is a NAMED type also tracks that name so
        // method calls on the binding resolve. Fall back to the literal's own field names
        // when the oracle has no entry (IR-only tests) — which never carry spreads.
        if let Expression::Record { fields, .. } = &declaration.value {
            // Field names in slot order: prefer the oracle's (it expands spreads and is
            // authoritative), else the literal's own names. Only a NAMED-type result also
            // records `var_named_types` so method calls on the binding resolve.
            let (field_names, named): (Vec<String>, Option<String>) =
                match self.oracle.expression_type(&declaration.value) {
                    Some(Type::Named { name, fields, .. }) => (
                        fields.iter().map(|(n, _)| n.clone()).collect(),
                        Some(name.clone()),
                    ),
                    Some(Type::Record(fields)) => {
                        (fields.iter().map(|(n, _)| n.clone()).collect(), None)
                    }
                    _ => (fields.iter().map(|(n, _)| n.clone()).collect(), None),
                };
            self.record_types
                .insert(declaration.name.clone(), field_names);
            if let Some(name) = named {
                self.var_named_types.insert(declaration.name.clone(), name);
            }
        }
        // A named-type instance (e.g. `u = User { ... }`) — remember its type so method calls
        // on `u` can resolve to the mangled `User_method` functions.
        if let Expression::Constructor {
            type_name, fields, ..
        } = &declaration.value
        {
            let field_names: Vec<String> = self
                .named_type_fields
                .get(type_name)
                .cloned()
                .unwrap_or_else(|| fields.iter().map(|(name, _)| name.clone()).collect());
            self.record_types
                .insert(declaration.name.clone(), field_names);
            self.var_named_types
                .insert(declaration.name.clone(), type_name.clone());
        }
        // Binding a function literal: remember its signature so a later `name(args)` can
        // recover the callee type for the indirect closure call (the closure value itself
        // does not encode it).
        if let Expression::Lambda {
            parameters,
            return_type,
            body,
            ..
        } = &declaration.value
        {
            let sig = self.closure_signature(parameters, return_type.as_ref(), body)?;
            self.closure_sigs.insert(declaration.name.clone(), sig);
        }

        // Remember the binding's Quilon type for overloaded-call argument mangling.
        let inferred_qty = self.infer_type(&declaration.value);
        // If the value is a named record (e.g. bound to a user operator overload's
        // result), track its type/fields so later `name.field` / method calls resolve.
        self.track_named_record_binding(&declaration.name, &inferred_qty);
        self.var_types
            .insert(declaration.name.clone(), inferred_qty);

        // A top-level binding becomes a global, and a global's initializer must already be
        // a constant — there is no code before `^` in which to compute one. Refused BEFORE
        // the value is generated: with the builder still pointing wherever the last
        // function left it, generating a computed value here appended its instructions to
        // that function (a call left it with a block that had no terminator, failing module
        // verification). The type checker reports this with a source location; this keeps
        // the invariant even for callers that build IR without checking first.
        if self.current_function.is_none()
            && !matches!(
                declaration.value,
                Expression::Number { .. }
                    | Expression::Bool { .. }
                    | Expression::Unit { .. }
                    | Expression::Lambda { .. }
            )
        {
            return Err(format!(
                "top-level '{}' must hold a Num, Bool or $ literal, or a function",
                declaration.name
            ));
        }

        let value = self.generate_expression(&declaration.value)?;

        if self.current_function.is_some() {
            let var_type = value.get_type();

            // Reassignment of an already-bound mutable local (`counter := counter + 1`):
            // store THROUGH the existing slot rather than allocating a fresh one. This is
            // what makes a `:=` capture escape-safe — the cell a closure shares is the
            // very cell later writes target — and it is equivalent to the old realloc for
            // ordinary straight-line code (reads always go through the latest slot).
            if declaration.mutable
                && let Some((slot, _)) = self.variables.get(&declaration.name).copied()
            {
                self.builder
                    .build_store(slot, value)
                    .map_err(ctx("Failed to build store"))?;
                return Ok(());
            }

            // A `:=` local captured by reference by some nested closure lives in a heap
            // GC cell (a "box"), so the closure and this frame share one mutable cell. Its
            // `variables` slot is the cell pointer; loads/stores work through it unchanged.
            let slot = if declaration.mutable && self.boxed_vars.contains(&declaration.name) {
                self.alloc_box(var_type)?
            } else {
                self.create_entry_block_alloca(&declaration.name, var_type)?
            };
            self.builder
                .build_store(slot, value)
                .map_err(ctx("Failed to build store"))?;
            self.variables
                .insert(declaration.name.clone(), (slot, var_type));
            // The binding's Quilon type is the one just recorded in `var_types` — borrow it
            // rather than keeping a separate clone alive across the whole binding.
            if let Some(qty) = self.var_types.get(&declaration.name) {
                self.declare_variable(&declaration.name, slot, qty, &declaration.span, None);
            }
        } else {
            // Global variable
            let global = self.module.add_global(
                value.get_type(),
                Some(AddressSpace::default()),
                &declaration.name,
            );
            global.set_initializer(&value);
        }

        Ok(())
    }

    pub(super) fn generate_function_declaration(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), String> {
        // The inert core.io print/eprint placeholder is never emitted (the compiler
        // lowers print/eprint to runtime intrinsics). A leaf `@` primitive (`@sleep`) is
        // likewise a corelib placeholder lowered to a runtime intrinsic at its call site.
        if declaration.is_inert_corelib_placeholder() || declaration.name.starts_with('@') {
            return Ok(());
        }

        // A function declared INSIDE another function (we are mid-emitting a body) is a
        // local declaration. If its body references enclosing locals it is a capturing
        // CLOSURE (lowered via the lambda machinery); otherwise it is a self-contained
        // local function, which we emit as a plain module function — that preserves
        // recursion (`fact = n => … fact(n-1) …`), since a closure value cannot refer to
        // itself before it exists. The choice is by ACTUAL captures, not syntax.
        if self.current_function.is_some() {
            // Emitting a nested function re-enters function emission, which sets and then
            // clears the outer function's TCO context (`self.tco`). Snapshot and restore
            // it so a nested tail-recursive function does not clobber the OUTER function's
            // active context — otherwise the outer tail walk resuming after this nested
            // declaration would panic ("generate_tail_expression without a TCO context").
            let saved_tco = self.tco.take();
            let parameter_names: Vec<String> = declaration
                .parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let outer: std::collections::HashSet<String> = self.variables.keys().cloned().collect();
            let captures = crate::ast::captures::lambda_free_idents(
                &parameter_names,
                &declaration.body,
                &outer,
            );
            let result = if !captures.is_empty() {
                self.generate_local_closure(declaration)
            } else {
                // No captures: emit a plain module function, but save/restore the
                // enclosing per-function frame and builder state around it, since
                // `emit_module_function` starts from an empty frame.
                let saved_block = self.builder.get_insert_block();
                let saved_function = self.current_function;
                let saved_frame = self.take_frame();

                let result = self.emit_module_function(declaration);

                self.restore_frame(saved_frame);
                self.current_function = saved_function;
                if let Some(block) = saved_block {
                    self.builder.position_at_end(block);
                }
                result
            };
            self.tco = saved_tco;
            return result;
        }

        self.emit_module_function(declaration)
    }

    /// Record a function-typed parameter's closure signature. Such a parameter arrives as a
    /// `{ ptr fn, ptr env }` closure value (its slot holds that struct), so a call to it in
    /// the body must dispatch through the indirect closure-call path — the env pointer is
    /// appended implicitly at the call site — exactly like a local closure binding. Every
    /// parameter-binding site (top-level functions, methods, and lifted closures) calls this
    /// so the three stay in lockstep. A non-function parameter is left untouched.
    pub(super) fn register_function_typed_parameter(
        &mut self,
        name: &str,
        qty: &Type,
    ) -> Result<(), String> {
        if let Type::Function {
            parameters,
            return_type,
        } = qty
        {
            let parameter_tys = parameters
                .iter()
                .map(|t| self.boundary_type(t))
                .collect::<Result<Vec<_>, _>>()?;
            let ret_ty = self.boundary_type(return_type)?;
            self.closure_sigs
                .insert(name.to_string(), (parameter_tys, ret_ty));
        }
        Ok(())
    }

    /// Emit `declaration` as a top-level/module function (internal linkage). Clears and
    /// repopulates the per-function emission state (`variables`, `closure_sigs`,
    /// `boxed_vars`, `var_types`); the entry point `^` gets the special f64-return /
    /// implicit-0 treatment. Used for true top-level functions and for non-capturing
    /// nested functions (which can recurse, unlike a closure value).
    pub(super) fn emit_module_function(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), String> {
        // Convert parameter types to LLVM types via the shared boundary rule: an ARRAY
        // parameter crosses as the `{ ptr, i64 }` VALUE struct (so `.size`/indexing work),
        // everything else via `type_to_llvm` (a record/sum parameter stays by pointer/struct).
        let parameter_types: Vec<BasicTypeEnum> = declaration
            .parameters
            .iter()
            .map(|p| self.boundary_type(&self.parameter_type(p)))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert return type. The entry point `^` always returns a Num exit code at
        // the LLVM level (the C `main` wrapper expects an f64), regardless of its body
        // type — so a side-effecting main can omit the trailing `0`.
        let return_type = if declaration.name == "^" {
            self.context.f64_type().into()
        } else {
            // An unannotated body defaults to `Num`, except a Unit (`$`) tail — e.g.
            // `log = m => print(m)` — which must be `i8`, not f64, or `build_return`
            // would emit `ret i8` into an f64 function and fail module verification.
            // The same boundary rule applies: an array return crosses as the value struct.
            let inferred =
                self.default_return_type(declaration.declared_return_type(), &declaration.body);
            self.boundary_type(&inferred)?
        };

        // Create function type - use a helper to convert BasicTypeEnum to BasicMetadataTypeEnum
        let fn_type = return_type.fn_type(
            &parameter_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        // Create the function. Use internal linkage so a Quilon function name never
        // collides with a C library / runtime symbol when the whole program is linked
        // into one native binary (AOT). For example core.io's `write` placeholder, or
        // a user function named `read`/`open`, would otherwise shadow libc and break
        // the runtime intrinsics. Only the generated `main` wrapper is exported.
        //
        // An overloaded member (operator-named, or one of several same-named defs) is
        // emitted under a per-signature MANGLED name so the members don't collide; each
        // call site dispatches to the matching mangled symbol by exact argument type.
        let symbol = if self.overloads.contains_key(&declaration.name) {
            let parameters = self.parameter_types(&declaration.parameters);
            mangle_overload(&declaration.name, &parameters)
        } else {
            declaration.name.clone()
        };
        let function = self.module.add_function(&symbol, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);
        self.current_function = Some(function);
        let saved_scope = self.begin_di_function(function, &declaration.name, &declaration.span);

        // Create entry block
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Store parameters in variables map
        self.take_frame(); // fresh frame: the previously emitted function's entries are dead
        // Which `:=` locals must be heap-boxed because a nested closure captures them.
        self.boxed_vars = self.compute_boxed_vars(&declaration.body);
        for (i, parameter) in declaration.parameters.iter().enumerate() {
            let llvm_parameter = function.get_nth_param(i as u32).unwrap();
            llvm_parameter.set_name(&parameter.name);

            // Allocate space for the parameter
            let parameter_type = llvm_parameter.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&parameter.name, parameter_type)?;
            self.builder
                .build_store(alloca, llvm_parameter)
                .map_err(ctx("Failed to build store"))?;

            self.variables
                .insert(parameter.name.clone(), (alloca, parameter_type));
            // Track the parameter's Quilon type for overloaded-call mangling, and so a
            // record/sum parameter's methods/fields resolve.
            let qty = self.parameter_type(parameter);
            if let Type::Named { name, .. } | Type::Sum { name, .. } = &qty {
                self.var_named_types
                    .insert(parameter.name.clone(), name.clone());
                if let Some(fields) = self.named_type_fields.get(name) {
                    self.record_types
                        .insert(parameter.name.clone(), fields.clone());
                }
            }
            self.register_function_typed_parameter(&parameter.name, &qty)?;
            self.declare_variable(
                &parameter.name,
                alloca,
                &qty,
                &parameter.span,
                Some((i + 1) as u32),
            );
            self.var_types.insert(parameter.name.clone(), qty);
        }

        // Guaranteed self-tail-call optimization: if the body returns a call to THIS
        // function in tail position, lower the recursion to a loop instead of a
        // stack-growing `call` + `ret`. Set up a loop header (branched to from the entry
        // block, after the parameter slots are populated) and a TCO context; a tail self-call
        // then rewrites the parameter slots and `br`s back here. The parameter allocas created
        // above are reused as the loop's mutable slots — there is no separate IR shape for
        // recursive vs. non-recursive functions beyond this header + the back-edge.
        let body_value = if self.body_has_self_tail_call(declaration, &symbol) {
            let parameter_slots: Vec<PointerValue<'ctx>> = declaration
                .parameters
                .iter()
                .map(|p| self.variables[&p.name].0)
                .collect();
            let header = self.context.append_basic_block(function, "tco_loop");
            self.builder
                .build_unconditional_branch(header)
                .map_err(ctx("Failed to build branch to loop header"))?;
            self.builder.position_at_end(header);
            self.tco = Some(Tco {
                self_symbol: symbol.clone(),
                function,
                parameter_slots,
                header,
            });
            // Emit the body in tail-aware mode. A `None` result means every tail exit was a
            // self-call (e.g. an unconditional `f(...)` body, or a match all of whose arms
            // tail-recurse): the function never falls through to a normal return, and
            // `generate_tail_expression` has already terminated the current block (with the
            // back-edge `br`, or an `unreachable`). In that case there is no `ret` to emit.
            let result = self.generate_tail_expression(&declaration.body)?;
            self.tco = None;
            match result {
                Some(v) => v,
                None => {
                    self.end_di_scope(saved_scope);
                    return Ok(());
                }
            }
        } else {
            self.generate_expression(&declaration.body)?
        };

        // Entry point `^`: if the body's value isn't a Num (f64) — e.g. a side-effecting
        // main ending in a Text/Bool/record expression — discard it and implicitly
        // return 0 (C `main`-style success). A Num body is used as the exit code as
        // usual. Scoped to `^`; ordinary functions return their body's actual type.
        let return_value: inkwell::values::BasicValueEnum =
            if declaration.name == "^" && !body_value.is_float_value() {
                self.context.f64_type().const_float(0.0).into()
            } else {
                body_value
            };
        self.builder
            .build_return(Some(&return_value))
            .map_err(ctx("Failed to build return"))?;

        self.end_di_scope(saved_scope);
        Ok(())
    }
}
