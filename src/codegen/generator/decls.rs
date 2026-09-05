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
            // Unannotated return type takes the checker's recorded type for the body
            // (e.g. `$` for a setter whose tail is an in-place field write).
            let inferred_ret =
                self.default_return_type(method.return_type.as_ref(), &method.body)?;
            let return_type = self.boundary_type(&inferred_ret)?;
            let fn_type = return_type.fn_type(&parameter_types, false);
            let method_fn = self.module.add_function(&mangled, fn_type, None);
            // Internal linkage: method symbols are module-private (see generate_function_declaration).
            method_fn.set_linkage(inkwell::module::Linkage::Internal);
        }

        // A type may be declared INSIDE a block (nested mid-emission of the enclosing
        // function's body), so suspend/resume around the method loop the same way a nested
        // plain function declaration does (see `generate_function_declaration`).
        let suspended = self.suspend_enclosing_function();

        let result = (|| {
            for method in methods {
                if crate::ast::is_operator_symbol(&method.name) {
                    self.emit_operator_member(type_name, method)?;
                } else {
                    self.generate_method(type_name, &field_names, method)?;
                }
            }
            Ok(())
        })();

        self.resume_enclosing_function(suspended);
        result
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
            // Track a record/sum-typed parameter's fields the same way a top-level
            // function's parameter does (`emit_module_function`), so `p.field` on a
            // method parameter resolves instead of hitting the "need type information"
            // fallback.
            if let Type::Named { name, .. } | Type::Sum { name, .. } = &qty {
                self.var_named_types
                    .insert(parameter.name.clone(), name.clone());
                if let Some(fields) = self.named_type_fields.get(name) {
                    self.record_types
                        .insert(parameter.name.clone(), fields.clone());
                }
            }
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
        // A top-level binding becomes a global, and a global's initializer must already be
        // a constant — there is no code before `^` in which to compute one. Checked FIRST,
        // ahead of every oracle read below: this is a pure AST-shape test needing no type
        // information, and it is the dedicated diagnostic for a caller that builds IR
        // without checking first (the type checker itself rejects the same program, with a
        // source location, before codegen ever sees it) — it must run before an oracle read
        // downstream has a chance to fail first with the generic "not type-checked" error.
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
        // Remember the binding's Quilon type for overloaded-call argument mangling. A
        // function-typed value — a lambda literal, a returned closure (`add5 =
        // adder(5)`), a captured/aliased closure — needs no separate bookkeeping for
        // `name(args)` to dispatch as an indirect call: the checker already recorded this
        // binding's type as `Type::Function` in the oracle, keyed by the CALL's own
        // identifier expression, which is what codegen reads at the call site.
        let inferred_qty =
            self.oracle_type(&declaration.value, "a variable declaration's value")?;
        // If the value is a named record (e.g. bound to a user operator overload's
        // result), track its type/fields so later `name.field` / method calls resolve.
        self.track_named_record_binding(&declaration.name, &inferred_qty);
        self.var_types
            .insert(declaration.name.clone(), inferred_qty);

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
            // Under `--debug`: narrow the scope from here on, so a debugger paused earlier
            // in the enclosing block does not list this local before it is bound. Everything
            // the rest of the block emits — including this variable's own `dbg.declare` —
            // moves into a fresh nested `DW_TAG_lexical_block` starting at the binding;
            // `generate_block`'s `end_di_scope` restores the enclosing scope once the block
            // ends. The value just computed above is unaffected — it was emitted under the
            // OUTER scope, before the variable existed.
            self.begin_di_lexical_block(&declaration.span);
            // Refresh the builder's current location to the new scope right away: an LLVM
            // lexical block with no instruction attributed to it gets dropped (variable and
            // all) rather than kept empty, and a binding as a block's LAST statement has no
            // further codegen of its own to carry the new scope otherwise — the caller's
            // next instruction (e.g. the function's `ret`) would silently inherit whatever
            // scope was current before this binding, orphaning both the block and the local.
            self.set_debug_loc(&declaration.span);
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
        // The same predicate gates the pre-declaration pass, so the two cannot drift.
        if !declaration.emits_module_function() {
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
                // No captures: emit a plain module function, but suspend/resume the
                // enclosing per-function frame, builder position, and debug location
                // around it, since `emit_module_function` starts from an empty frame.
                let suspended = self.suspend_enclosing_function();
                let result = self.emit_module_function(declaration);
                self.resume_enclosing_function(suspended);
                result
            };
            self.tco = saved_tco;
            return result;
        }

        self.emit_module_function(declaration)
    }

    /// Declare `declaration`'s LLVM function — signature and symbol, no body. What the
    /// pre-declaration pass in [`CodeGenerator::generate`] runs over every reachable
    /// top-level function, so a body emitted earlier in item order can call one emitted
    /// later (a `core.text` implementation behind a member call, a harness function ahead
    /// of the module implementing what it uses).
    ///
    /// Use internal linkage so a Quilon function name never collides with a C library /
    /// runtime symbol when the whole program is linked into one native binary (AOT). For
    /// example core.io's `write` placeholder, or a user function named `read`/`open`,
    /// would otherwise shadow libc and break the runtime intrinsics. Only the generated
    /// `main` wrapper is exported.
    ///
    /// An overloaded member (operator-named, or one of several same-named defs) is
    /// declared under a per-signature MANGLED name so the members don't collide; each
    /// call site dispatches to the matching mangled symbol by exact argument type.
    pub(super) fn declare_module_function(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<inkwell::values::FunctionValue<'ctx>, String> {
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
            // An unannotated body takes the checker's recorded type — e.g. `$` (i8) for
            // `log = m => print(m)`, not the historical `Num`/f64 guess, which would emit
            // `ret i8` into an f64 function and fail module verification. The same
            // boundary rule applies: an array return crosses as the value struct.
            let inferred =
                self.default_return_type(declaration.declared_return_type(), &declaration.body)?;
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

        let function = self
            .module
            .add_function(&self.module_symbol(declaration), fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);
        Ok(function)
    }

    /// The symbol `declaration` is emitted under: its name, or the per-signature mangled
    /// form for an overload-set member. Shared by declaration and by the TCO analysis,
    /// which recognizes a self-call by this symbol.
    fn module_symbol(&self, declaration: &FunctionDeclaration) -> String {
        if self.overloads.contains_key(&declaration.name) {
            let parameters = self.parameter_types(&declaration.parameters);
            mangle_overload(&declaration.name, &parameters)
        } else {
            declaration.name.clone()
        }
    }

    /// Emit `declaration` as a top-level/module function (internal linkage). Clears and
    /// repopulates the per-function emission state (`variables`, `boxed_vars`,
    /// `var_types`); the entry point `^` gets the special f64-return /
    /// implicit-0 treatment. Used for true top-level functions and for non-capturing
    /// nested functions (which can recurse, unlike a closure value).
    pub(super) fn emit_module_function(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), String> {
        // A top-level function was declared by the pre-declaration pass — take that
        // declaration (keyed by the item's span, its identity) and fill its body in. A
        // NESTED plain function was not pre-declared and is declared here; keying by span
        // (not symbol) is what keeps a nested function shadowing a top-level name from
        // stealing the top-level declaration.
        let function = match self.predeclared_functions.remove(&declaration.span) {
            Some(function) => function,
            None => self.declare_module_function(declaration)?,
        };
        let symbol = self.module_symbol(declaration);
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
        let body_value = if self.body_has_self_tail_call(declaration, &symbol)? {
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
