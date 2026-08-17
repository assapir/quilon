//! Checking declarations: the program walk, type/record declarations and their methods
//! (including which of them mutate the receiver), bindings, functions, and lambdas.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;
use std::rc::Rc;

impl TypeChecker {
    /// Type-check `program` and, on success, return the **type oracle** (`TypeTable`):
    /// every expression's inferred type keyed by its source span. Codegen consumes this
    /// to recover precise element/field/match-result types at read sites. The table is
    /// taken (moved) out of the checker, so a checker is single-use per program.
    pub fn check_program(&mut self, program: &Program) -> Result<TypeTable, TypeError> {
        // Pre-pass: find the names that form an overload set — operator-named
        // definitions, or any name with 2+ top-level function definitions. These
        // dispatch by exact argument type instead of through a single `env` binding.
        let mut fn_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
            {
                *fn_counts.entry(decl.name.as_str()).or_insert(0) += 1;
            }
        }
        // A name forms an overload set if it is operator-named, has 2+ definitions, OR
        // already has a built-in overload set (e.g. `print`/`eprint` — a user
        // definition of one ADDS an overload, it does not shadow the builtins).
        // `^` (entry point) is never an overload set, even if (erroneously) repeated.
        self.overloaded_names = fn_counts
            .iter()
            .filter(|(name, count)| {
                (crate::ast::is_operator_symbol(name)
                    || **count > 1
                    || self.overloads.contains_key(**name))
                    && **name != "^"
            })
            .map(|(name, _)| name.to_string())
            .collect();

        // Names resolve top to bottom: an overload member joins its set as its own
        // definition is reached, NOT up front, so a call can only pick a member defined
        // above it. Registering just before the member's body is checked still lets that
        // body call itself (the same way a plain function's definition is in scope for
        // its own body); what it rules out is a call reaching forward to a definition
        // below — which the checker used to accept and codegen then had no symbol for.
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && self.overloaded_names.contains(&decl.name)
                && !decl.is_inert_io_placeholder()
            {
                self.register_overload_decl(decl)?;
            }
            self.check_item(item)?;
        }

        // Every call has been resolved by now, so an overload member still missing its
        // return annotation is one nothing calls — reported at its definition.
        self.report_unannotated_overload_member()?;

        // Validate the `^` entry point's parameter signature up front, so `quilon check`
        // and `quilon run`/`build` all reject an unsupported form with the SAME clear
        // diagnostic (rather than passing the check and failing later in codegen).
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && decl.name == "^"
            {
                Self::check_entry_point_signature(decl)?;
            }
        }

        Ok(std::mem::take(&mut self.type_table))
    }

    /// The `^` entry point may only take one of these parameter shapes (checked by
    /// TYPE, not by parameter name): `()`, `(args :: []Text)`,
    /// `(args :: []Text, env :: [][]Text)`, or the legacy `(argc :: Num, argv :: Num)`.
    /// The runtime builds `Text`/`[]Text` elements for argv/env, so a differently-typed
    /// array (e.g. `[]Num`) must be rejected rather than silently handed mis-sized
    /// elements. An unannotated parameter defaults to `Num` (matching codegen), so
    /// `^(x)` is the legacy shape only if it has exactly two such parameters.
    pub(super) fn check_entry_point_signature(decl: &FunctionDecl) -> Result<(), TypeError> {
        let params: Vec<Type> = decl
            .params
            .iter()
            .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
            .collect();
        let text_array = Type::Array(Box::new(Type::Text));
        let text_pairs = Type::Array(Box::new(text_array.clone()));
        let ok = match params.as_slice() {
            [] => true,
            [a] => *a == text_array,
            [a, b] => {
                (*a == text_array && *b == text_pairs) || (*a == Type::Num && *b == Type::Num)
            }
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(TypeError::InvalidEntryPointSignature {
                got: params,
                span: decl.span.clone(),
            })
        }
    }

    pub(super) fn check_item(&mut self, item: &Item) -> Result<(), TypeError> {
        match item {
            Item::VarDecl(decl) => self.check_var_decl(decl),
            Item::FunctionDecl(decl) => self.check_function_decl(decl),
            Item::TypeDecl(decl) => self.check_type_decl(decl),
        }
    }

    pub(super) fn check_type_decl(&mut self, decl: &crate::ast::TypeDecl) -> Result<(), TypeError> {
        use crate::ast::{Type, TypeDef};

        // Build the type from the definition
        let type_value = match &decl.type_def {
            TypeDef::Sum(variants) => {
                // Payloads are built-in types only — `Num` / `Text` / `Bool` / `$`
                // (Unit). No type variables, no nesting of other user types. (LOCKED
                // design.) `$` is the canonical "no meaningful value" payload, e.g.
                // `Ok($)`.
                for variant in variants {
                    for field in &variant.fields {
                        if !matches!(field, Type::Num | Type::Text | Type::Bool | Type::Unit) {
                            return Err(TypeError::TypeMismatch {
                                expected: Box::new(Type::Num),
                                got: Box::new(field.clone()),
                                span: decl.span.clone(),
                            });
                        }
                    }
                }

                // A sum type's payload slots have ONE shared LLVM representation per
                // position (sized to the widest variant — see codegen). So at each
                // payload position, every variant that has a field there must agree on
                // the type, EXCEPT `$` (Unit), which is zero-sized and stored nowhere,
                // so it may coexist with a concrete type at the same position (e.g.
                // `A($) / B(Num)`). Heterogeneous concrete types at one position (e.g.
                // `A(Num) / B(Text)`) would miscompile, so reject them up front.
                let max_arity = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
                for pos in 0..max_arity {
                    let mut concrete: Option<&Type> = None;
                    for variant in variants {
                        if let Some(field) = variant.fields.get(pos)
                            && *field != Type::Unit
                        {
                            match concrete {
                                None => concrete = Some(field),
                                Some(prev) if prev != field => {
                                    return Err(TypeError::TypeMismatch {
                                        expected: Box::new(prev.clone()),
                                        got: Box::new(field.clone()),
                                        span: decl.span.clone(),
                                    });
                                }
                                Some(_) => {}
                            }
                        }
                    }
                }

                // Variant (constructor) names must be unique per scope: both within
                // this declaration and against any already-registered variant.
                let mut seen = std::collections::HashSet::new();
                for variant in variants {
                    if !seen.insert(variant.name.clone())
                        || self.sum_variant_owner(&variant.name).is_some()
                    {
                        return Err(TypeError::DuplicateDefinition {
                            name: variant.name.clone(),
                            span: decl.span.clone(),
                        });
                    }
                }

                let sum_type = Type::Sum {
                    name: decl.name.clone(),
                    variants: variants.clone(),
                };
                // Register the sum type for constructor lookup
                self.sum_types.insert(decl.name.clone(), sum_type.clone());

                // Bind each nullary variant as a value of the sum type, so a bare
                // `Red` resolves as an expression. (Variants with payloads are
                // resolved as constructor calls in `check_call`.)
                for variant in variants {
                    if variant.fields.is_empty() {
                        self.env.define(
                            variant.name.clone(),
                            sum_type.clone(),
                            false,
                            decl.span.clone(),
                        )?;
                    }
                }
                sum_type
            }
            TypeDef::Record { fields, methods } => {
                // Infer which methods mutate the receiver in place ("setters"), so
                // a later call-site can require a `:=` receiver. A method is a setter
                // iff its body writes `it.field := …` or calls a sibling setter on
                // `it`. The latter is resolved to a fixpoint (setters calling setters).
                self.infer_setter_methods(&decl.name, methods);

                // The declaration itself, built once: every method's `it` binding and the
                // registered type are the same record, so they share one copy of it.
                let record_fields = Rc::new(fields.clone());
                let method_names: Rc<Vec<String>> =
                    Rc::new(methods.iter().map(|m| m.name.clone()).collect());

                // Type-check each method
                for method in methods {
                    // Create a new scope for the method
                    self.env.push_scope();

                    // Bind implicit "it" parameter to the struct type
                    let struct_type = Type::Named {
                        name: decl.name.clone(),
                        fields: Rc::clone(&record_fields),
                        methods: Rc::clone(&method_names),
                    };

                    self.env
                        .define("it".to_string(), struct_type, false, method.span.clone())?;

                    // Bind method parameters
                    for param in &method.params {
                        let param_type = param.type_annotation.clone().unwrap_or(Type::Num); // Default to Num if no type annotation
                        self.env.define(
                            param.name.clone(),
                            param_type,
                            false,
                            param.span.clone(),
                        )?;
                    }

                    // Type-check method body
                    let body_type = self.infer_expr(&method.body)?;

                    // Check return type if specified, and resolve the method's actual
                    // result type: the annotation when present, otherwise the inferred
                    // body type. Storing the *resolved* type (not the raw annotation)
                    // keeps call sites in agreement with codegen — e.g. an unannotated
                    // setter whose body is a field write yields `$` (Unit), not Num.
                    let resolved_return_type = if let Some(ref return_type) = method.return_type {
                        self.check_type_compatibility(return_type, &body_type, &method.span)?;
                        return_type.clone()
                    } else {
                        body_type
                    };

                    // The render operator `` ` `` must render to `Text` and take only its
                    // implicit `it` receiver (interpolation/`print` call it with no extra
                    // arguments).
                    if method.name == "`" {
                        if !method.params.is_empty() {
                            return Err(TypeError::InvalidBuiltinArgument {
                                message: "the `` ` `` render operator takes no parameters (only its implicit `it` receiver)".to_string(),
                                span: method.span.clone(),
                            });
                        }
                        if resolved_return_type != Type::Text {
                            return Err(TypeError::InvalidBuiltinArgument {
                                message: format!(
                                    "the `` ` `` render operator must return Text, but returns {}",
                                    type_label(&resolved_return_type)
                                ),
                                span: method.span.clone(),
                            });
                        }
                    }

                    self.env.pop_scope();

                    // Store method for later lookup
                    self.methods.insert(
                        (decl.name.clone(), method.name.clone()),
                        (
                            method.params.clone(),
                            Some(resolved_return_type),
                            method.body.clone(),
                        ),
                    );
                }

                // Create a Named type with methods
                Type::Named {
                    name: decl.name.clone(),
                    fields: record_fields,
                    methods: method_names,
                }
            }
        };

        // Register the type name in the environment
        // For now, we treat types as values (not ideal but works)
        self.env
            .define(decl.name.clone(), type_value, false, decl.span.clone())?;

        Ok(())
    }

    /// Populate `self.setter_methods` for `type_name`'s methods. A method is a
    /// setter iff its body mutates the receiver: a direct `it.field := …`, or a
    /// call to a sibling method on `it` that is itself a setter. Because setters
    /// can call setters, this is iterated to a fixpoint over the method set.
    pub(super) fn infer_setter_methods(
        &mut self,
        type_name: &str,
        methods: &[crate::ast::MethodDecl],
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for method in methods {
                let key = (type_name.to_string(), method.name.clone());
                if self.setter_methods.contains(&key) {
                    continue;
                }
                if self.body_mutates_receiver(type_name, &method.body) {
                    self.setter_methods.insert(key);
                    changed = true;
                }
            }
        }
    }

    /// Does `expr` (a method body) mutate the receiver `it`? True if it contains a
    /// field write rooted at `it` (`it.field := …`, `it.a.b := …`) or a call to a
    /// sibling method already known to be a setter, applied to `it`.
    pub(super) fn body_mutates_receiver(&self, type_name: &str, expr: &Expr) -> bool {
        match expr {
            Expr::FieldAssign { target, value, .. } => {
                Self::field_path_root_name(target).as_deref() == Some("it")
                    || self.body_mutates_receiver(type_name, value)
            }
            Expr::Call { func, args, .. } => {
                // `it.setter(...)` desugars to `setter(it, ...)`: a sibling setter
                // applied to `it` propagates "mutating" to the caller.
                if let Expr::Ident { name, .. } = func.as_ref()
                    && args.first().is_some_and(
                        |recv| matches!(recv, Expr::Ident { name, .. } if name == "it"),
                    )
                    && self
                        .setter_methods
                        .contains(&(type_name.to_string(), name.clone()))
                {
                    return true;
                }
                args.iter()
                    .any(|a| self.body_mutates_receiver(type_name, a))
            }
            Expr::Block { stmts, .. } => stmts.iter().any(|s| match s {
                crate::ast::Statement::Expr(e) => self.body_mutates_receiver(type_name, e),
                crate::ast::Statement::Item(Item::VarDecl(d)) => {
                    self.body_mutates_receiver(type_name, &d.value)
                }
                crate::ast::Statement::Item(_) => false,
            }),
            Expr::If {
                cond, then, else_, ..
            } => {
                self.body_mutates_receiver(type_name, cond)
                    || self.body_mutates_receiver(type_name, then)
                    || self.body_mutates_receiver(type_name, else_)
            }
            Expr::Match { expr, arms, .. } => {
                self.body_mutates_receiver(type_name, expr)
                    || arms
                        .iter()
                        .any(|a| self.body_mutates_receiver(type_name, &a.body))
            }
            Expr::BinOp { left, right, .. } => {
                self.body_mutates_receiver(type_name, left)
                    || self.body_mutates_receiver(type_name, right)
            }
            Expr::UnaryOp { expr, .. } => self.body_mutates_receiver(type_name, expr),
            Expr::Pipeline { left, right, .. } => {
                self.body_mutates_receiver(type_name, left)
                    || self.body_mutates_receiver(type_name, right)
            }
            _ => false,
        }
    }

    /// The name of the variable at the root of a field-access path, if any:
    /// `a.b.c` -> `Some("a")`. Returns `None` if the root isn't a plain ident.
    pub(super) fn field_path_root_name(target: &Expr) -> Option<String> {
        match target {
            Expr::FieldAccess { expr, .. } => Self::field_path_root_name(expr),
            Expr::Ident { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// If a mutation rooted at `receiver` would write through an *immutable*
    /// binding, return that binding's name; otherwise `None`. A `:=`-bound
    /// receiver and the method receiver `it` (whose mutability is enforced at the
    /// outer call site) are both allowed. Shared by the field-write and
    /// setter-call mutability gates so they can never diverge.
    pub(super) fn immutable_mutation_root(&self, receiver: &Expr) -> Option<String> {
        let name = Self::field_path_root_name(receiver)?;
        if name != "it" && !self.env.is_mutable(&name) {
            Some(name)
        } else {
            None
        }
    }

    pub(super) fn check_var_decl(&mut self, decl: &VarDecl) -> Result<(), TypeError> {
        // Infer or check the type of the value
        let value_type = self.infer_expr(&decl.value)?;

        // If type annotation exists, check it matches
        let final_type = if let Some(ref annotated_type) = decl.type_annotation {
            let annotated_type = self.resolve_type(annotated_type);
            self.check_type_compatibility(&annotated_type, &value_type, &decl.span)?;
            annotated_type
        } else {
            value_type
        };

        if decl.mutable {
            // `:=` — reassign if the name is already bound, otherwise a new mutable binding.
            if let Some(existing_type) = self.env.get_type(&decl.name) {
                if !self.env.is_mutable(&decl.name) {
                    return Err(TypeError::ImmutableAssignment {
                        name: decl.name.clone(),
                        span: decl.span.clone(),
                    });
                }
                // Reassignment: the new value must match the binding's type.
                self.check_type_compatibility(&existing_type, &final_type, &decl.span)?;
                Ok(())
            } else {
                self.env
                    .define(decl.name.clone(), final_type, true, decl.span.clone())
            }
        } else {
            // `=` — immutable binding; a same-scope duplicate is a DuplicateDefinition.
            self.env
                .define(decl.name.clone(), final_type, false, decl.span.clone())
        }
    }

    pub(super) fn check_function_decl(&mut self, decl: &FunctionDecl) -> Result<(), TypeError> {
        // The inert core.io `print`/`eprint` placeholder is fully provided by the
        // compiler as a built-in overload; ignore its declaration entirely.
        if decl.is_inert_io_placeholder() {
            return Ok(());
        }

        // Build function type from parameters and return type
        let param_types: Vec<Type> = decl
            .params
            .iter()
            .map(|p| {
                p.type_annotation
                    .as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(Type::Num)
            })
            .collect();

        // For recursion support, we need to add the function to the environment
        // BEFORE checking its body. We'll use the annotated return type if available,
        // or default to Num (which we'll verify later)
        let preliminary_return_type = decl
            .return_type
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Type::Num);

        // An overloaded member (operator-named, or one of 2+ same-named defs) is NOT
        // a single `env` binding — its signature already lives in the overload set
        // (registered in the pre-pass). We only type-check its body here, then refine
        // that member's return type from the inferred body when it wasn't annotated.
        let is_overloaded = self.overloaded_names.contains(&decl.name);

        if !is_overloaded {
            let func_type = Type::Function {
                params: param_types.clone(),
                return_type: Box::new(preliminary_return_type.clone()),
            };
            // Define the function in current scope BEFORE checking body (enables recursion)
            self.env
                .define(decl.name.clone(), func_type, false, decl.span.clone())?;
        }

        // Push scope for body type checking
        self.env.push_scope();

        // Add parameters to scope
        for (param, param_type) in decl.params.iter().zip(param_types.iter()) {
            self.env.define(
                param.name.clone(),
                param_type.clone(),
                false,
                param.span.clone(),
            )?;
        }

        // Check body and infer return type
        let body_type = self.infer_expr(&decl.body)?;

        self.env.pop_scope();

        // Verify the return type matches if annotated
        if let Some(ref annotated_type) = decl.return_type {
            let annotated_type = self.resolve_type(annotated_type);
            self.check_type_compatibility(&annotated_type, &body_type, &decl.span)?;
            // A GENERIC return annotation — in practice only `-> Result`, whose
            // `Ok(T)`/`NotOk(E)` payload slots are type variables the language cannot
            // otherwise name — is refined to the inferred body type, so a caller of
            // `f() -> Result` that binds the payload sees its real type (`Ok("x")` =>
            // `Text`) instead of the opaque `Generic`. The body type is the ground
            // truth and is never less concrete than a generic annotation (an
            // unconstructed variant may keep a `Generic` slot, e.g. the `NotOk` of a
            // function that only returns `Ok(text)` — still strictly more informative).
            // This mirrors `check_match` preferring a concrete arm type over a generic
            // one and introduces no generics (the annotation still stands as the
            // compatibility check). A concrete annotation is left exactly as written,
            // and an overloaded member keeps its per-member registered return.
            if !is_overloaded && annotated_type.contains_generic() {
                let refined = Type::Function {
                    params: param_types.clone(),
                    return_type: Box::new(body_type.clone()),
                };
                let _ = self.env.update_type(&decl.name, refined);
            }
        } else if is_overloaded {
            // An overload member's return type is its annotation, never its inferred body
            // type. Adopting the body type here would make the member's signature depend
            // on where a call sits relative to the definition — a call above it would see
            // one type and a call below it another — which is precisely the order
            // dependence the annotation requirement removes. The omission is reported
            // instead, at the call or at the definition.
        } else if body_type != preliminary_return_type {
            // Update the function type with the inferred return type
            let correct_func_type = Type::Function {
                params: param_types,
                return_type: Box::new(body_type.clone()),
            };
            let _ = self.env.update_type(&decl.name, correct_func_type);
        }

        Ok(())
    }

    /// Type-check a function-literal (lambda) expression and return its
    /// `Type::Function`. The lambda's body is checked in a fresh scope layered over the
    /// enclosing environment, so it may reference (capture) outer bindings — `Environment`
    /// scopes are a stack and `lookup` walks outward. Capture-by-value vs by-reference is
    /// decided downstream (codegen) from each captured name's binding operator; the
    /// checker only needs the outer binding to be visible and concrete.
    ///
    /// Closures are MONOMORPHIC in M3: parameters are concrete-typed (annotated, else the
    /// `Num` default, matching top-level functions) and captured values are concrete. The
    /// language has no type variables, so there is nothing polymorphic to capture; generic
    /// closures + defunctionalization are deferred to M4.
    pub(super) fn check_lambda(
        &mut self,
        params: &[Param],
        return_type: Option<&Type>,
        body: &Expr,
    ) -> Result<Type, TypeError> {
        let param_types: Vec<Type> = params
            .iter()
            .map(|p| {
                p.type_annotation
                    .as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(Type::Num)
            })
            .collect();

        self.env.push_scope();
        for (param, param_type) in params.iter().zip(param_types.iter()) {
            self.env.define(
                param.name.clone(),
                param_type.clone(),
                false,
                param.span.clone(),
            )?;
        }
        let body_type = self.infer_expr(body)?;
        self.env.pop_scope();

        // Honor an explicit `-> Type` annotation; otherwise the body's inferred type is
        // the return type.
        let ret = match return_type {
            Some(annotated) => {
                let annotated = self.resolve_type(annotated);
                self.check_type_compatibility(&annotated, &body_type, body.span())?;
                annotated
            }
            None => body_type,
        };

        Ok(Type::Function {
            params: param_types,
            return_type: Box::new(ret),
        })
    }
}
