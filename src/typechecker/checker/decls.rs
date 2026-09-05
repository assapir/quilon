//! Checking declarations: the program walk, type/record declarations and their methods
//! (including which of them mutate the receiver), bindings, functions, and lambdas.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;
use crate::ast::walk::try_for_each_subexpression;
use std::ops::ControlFlow;
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
            if let Item::FunctionDeclaration(declaration) = item
                && !declaration.is_inert_corelib_placeholder()
            {
                *fn_counts.entry(declaration.name.as_str()).or_insert(0) += 1;
            }
        }
        // A name forms an overload set if it is operator-named, has 2+ definitions, OR the
        // compiler provides it. Post-link only the bare internal primitives (`__exit`,
        // `__color_enabled`, `__test_*`) can match a user definition — a user definition
        // of one of THOSE adds an overload beside the intrinsic; the module-qualified
        // built-ins (`core.io.print`, `core.time.now`) are names no user file can declare,
        // so their sets stay closed. Codegen asks the same question, so the two passes
        // agree on what a single definition of such a name is.
        // `^` (entry point) is never an overload set, even if (erroneously) repeated.
        self.overloaded_names = fn_counts
            .iter()
            .filter(|(name, count)| {
                (crate::ast::is_operator_symbol(name)
                    || **count > 1
                    || crate::ast::is_compiler_provided_name(name))
                    && **name != "^"
            })
            .map(|(name, _)| name.to_string())
            .collect();

        // Names resolve top to bottom: an overload member joins its set as its own
        // definition is reached, NOT up front, so a call can only pick a member defined
        // above it. Registering just before the member's body is checked still lets that
        // body call itself (the same way a plain function's definition is in scope for
        // its own body); what it rules out is a call reaching forward to a definition
        // below, which codegen has no symbol for.
        for item in &program.items {
            // A program may not bind a reserved name (`ast::reserved_for`); the corelib is
            // where some of them are defined, so its declarations are exempt.
            self.env.enforce_reserved_names = !matches!(
                item,
                Item::FunctionDeclaration(declaration) if declaration.from_corelib
            );
            if let Item::FunctionDeclaration(declaration) = item
                && self.overloaded_names.contains(&declaration.name)
                && !declaration.is_inert_corelib_placeholder()
            {
                // An operator overload now lives inside a type (as a member), not at the
                // top level: `it` is the left operand. Reject a top-level operator-symbol
                // definition and point to the member form. (Ordinary function overload
                // sets are unaffected — only operator symbols move.)
                if crate::ast::is_operator_symbol(&declaration.name) {
                    return Err(TypeError::OperatorMustBeMember {
                        operator: declaration.name.clone(),
                        span: declaration.span.clone(),
                    });
                }
                self.register_overload_declaration(declaration)?;
            }
            self.check_item(item, Nesting::TopLevel)?;
        }

        // Every call has been resolved by now, so an overload member still missing its
        // return annotation is one nothing calls — reported at its definition.
        self.report_unannotated_overload_member()?;

        // Validate the `^` entry point's parameter signature up front, so `quilon check`
        // and `quilon run`/`build` all reject an unsupported form with the SAME clear
        // diagnostic (rather than passing the check and failing later in codegen).
        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) if declaration.name == "^" => {
                    Self::check_entry_point_signature(declaration)?;
                }
                // Same reason: a top-level binding that has to be computed passes the
                // check and then breaks codegen from the inside.
                Item::VariableDeclaration(declaration) => Self::check_global_binding(declaration)?,
                _ => {}
            }
        }

        Ok(std::mem::take(&mut self.type_table))
    }

    /// Take the matcher hover side-table (see [`MatcherHoverTable`]) built alongside the
    /// type oracle. Call after `check_program` succeeds — a language server's hover reads
    /// it beside `types` to answer a matcher span with its signature rather than the
    /// enclosing `assert`/`expect` call's `$`.
    pub fn take_matcher_hovers(&mut self) -> MatcherHoverTable {
        std::mem::take(&mut self.matcher_hovers)
    }

    /// The `^` entry point may only take one of these parameter shapes (checked by
    /// TYPE, not by parameter name): `()`, `(args :: []Text)`, or
    /// `(args :: []Text, env :: [|Text => Text|])`.
    /// The runtime builds `Text` args and a `Text => Text` env Map, so a differently-typed
    /// array (e.g. `[]Num`) must be rejected rather than silently handed mis-sized
    /// elements. `^` is checked like any other function first (in the `check_item` pass
    /// above), so an unannotated parameter has already been rejected as
    /// `UnannotatedParameter` by the time this runs — every parameter here is annotated.
    pub(super) fn check_entry_point_signature(
        declaration: &FunctionDeclaration,
    ) -> Result<(), TypeError> {
        let parameters: Vec<Type> = (0..declaration.parameters.len())
            .map(|i| {
                declaration
                    .parameter_type(i)
                    .cloned()
                    .expect("^'s parameters are annotated: checked in check_function_declaration")
            })
            .collect();
        let text_array = Type::Array(Box::new(Type::Text));
        let text_map = Type::Map(Box::new(Type::Text), Box::new(Type::Text));
        let ok = match parameters.as_slice() {
            [] => true,
            [a] => *a == text_array,
            [a, b] => *a == text_array && *b == text_map,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(TypeError::InvalidEntryPointSignature {
                got: parameters,
                span: declaration.span.clone(),
            })
        }
    }

    /// A top-level binding becomes a global, and a global's initializer has to be a
    /// constant: there is no code that runs before `^` in which to compute one. So the
    /// value may be a `Num`, `Bool` or `$` literal, or a function (a lambda binding is
    /// emitted as a function, not as an initializer) — and nothing else.
    ///
    /// Checked here because codegen cannot report it. Codegen builds the value's
    /// instructions wherever the builder was last left, so `x = 1 + 2` surfaces as the
    /// internal `Failed to build add: UnsetPosition`, and `x = f(1)` appends a call to the
    /// function emitted before it — leaving a block with no terminator that fails module
    /// verification. Neither says anything about the binding, and both reach codegen only
    /// after passing `quilon check`.
    pub(super) fn check_global_binding(declaration: &VariableDeclaration) -> Result<(), TypeError> {
        let constant = matches!(
            declaration.value,
            Expression::Number { .. }
                | Expression::Bool { .. }
                | Expression::Unit { .. }
                | Expression::Lambda { .. }
        );
        if constant {
            Ok(())
        } else {
            Err(TypeError::ComputedGlobalBinding {
                name: declaration.name.clone(),
                span: declaration.span.clone(),
            })
        }
    }

    /// Check one declaration. `nesting` says whether it sits at the top level of a module
    /// or inside some body — which is what decides whether it may take a `Site` parameter
    /// (see [`Self::reject_unfillable_site_parameters`]). It is an argument rather than checker
    /// state so that a body-descending path cannot forget to set it.
    pub(super) fn check_item(&mut self, item: &Item, nesting: Nesting) -> Result<(), TypeError> {
        match item {
            Item::VariableDeclaration(declaration) => self.check_variable_declaration(declaration),
            Item::FunctionDeclaration(declaration) => {
                self.check_function_declaration(declaration, nesting)
            }
            Item::TypeDeclaration(declaration) => self.check_type_declaration(declaration),
        }
    }

    pub(super) fn check_type_declaration(
        &mut self,
        declaration: &crate::ast::TypeDeclaration,
    ) -> Result<(), TypeError> {
        use crate::ast::{SumVariant, Type, TypeDefinition};

        // Build the type from the definition
        // A setter is DECLARED, with `:=` — the binding operator means here what it means
        // everywhere else. Records only: a sum's methods cannot mutate `it` (no fields to
        // write), the parser rejects `:=` on them, and running the verifier over one would
        // answer a field write with setter advice instead of the truth, which is that a
        // sum has no such field.
        if let TypeDefinition::Record { methods, .. } = &declaration.type_definition {
            self.check_method_mutation_contracts(&declaration.name, methods)?;
        }

        let type_value = match &declaration.type_definition {
            TypeDefinition::Sum { variants, .. } => {
                // Resolve and validate each variant's payload types. A payload is either a
                // built-in type — `Num` / `Text` / `Bool` / `$` (Unit) — or a NAMED
                // composite that resolves to an already-declared RECORD (no hoisting, so it
                // must appear above this declaration). The resolved record carries its
                // fields, so a later `match Box(p)` binds `p` at its real type and reads
                // `p.field`. Anything else — an array, a type variable, a nested sum, or an
                // unknown/non-record name — is rejected. `$` is the canonical "no meaningful
                // value" payload, e.g. `Ok($)`.
                let mut resolved_variants = Vec::with_capacity(variants.len());
                for variant in variants {
                    let mut fields = Vec::with_capacity(variant.fields.len());
                    for field in &variant.fields {
                        let resolved = self.resolve_type(field);
                        let acceptable = match &resolved {
                            Type::Num | Type::Text | Type::Bool | Type::Unit => true,
                            // A named payload must resolve to a declared RECORD.
                            // `resolve_type` maps a sum name to `Type::Sum` (so nested sums
                            // fall through to the reject arm below) and leaves an unknown
                            // name as a field-less `Named`; the env lookup distinguishes a
                            // real record — of any field/method count — from that unknown.
                            Type::Named { name, .. } => {
                                matches!(self.env.get_type(name), Some(Type::Named { .. }))
                            }
                            _ => false,
                        };
                        if !acceptable {
                            return Err(TypeError::TypeMismatch {
                                expected: Box::new(Type::Num),
                                got: Box::new(field.clone()),
                                span: declaration.span.clone(),
                            });
                        }
                        fields.push(resolved);
                    }
                    resolved_variants.push(SumVariant {
                        name: variant.name.clone(),
                        fields,
                    });
                }

                // A sum type's payload slots have ONE shared LLVM representation per
                // position (sized to the widest variant — see codegen). So at each
                // payload position, every variant that has a field there must agree on
                // the type, EXCEPT `$` (Unit), which is zero-sized and stored nowhere,
                // so it may coexist with a concrete type at the same position (e.g.
                // `A($) / B(Num)`). Heterogeneous concrete types at one position (e.g.
                // `A(Num) / B(Text)`) would miscompile, so reject them up front.
                let max_arity = resolved_variants
                    .iter()
                    .map(|v| v.fields.len())
                    .max()
                    .unwrap_or(0);
                for pos in 0..max_arity {
                    let mut concrete: Option<&Type> = None;
                    for variant in &resolved_variants {
                        if let Some(field) = variant.fields.get(pos)
                            && *field != Type::Unit
                        {
                            match concrete {
                                None => concrete = Some(field),
                                Some(prev) if prev != field => {
                                    return Err(TypeError::TypeMismatch {
                                        expected: Box::new(prev.clone()),
                                        got: Box::new(field.clone()),
                                        span: declaration.span.clone(),
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
                for variant in &resolved_variants {
                    if !seen.insert(variant.name.clone())
                        || self.sum_variant_owner(&variant.name).is_some()
                    {
                        return Err(TypeError::DuplicateDefinition {
                            name: variant.name.clone(),
                            span: declaration.span.clone(),
                        });
                    }
                }

                let sum_type = Type::Sum {
                    name: declaration.name.clone(),
                    variants: resolved_variants,
                };
                // Register the sum type for constructor lookup
                self.sum_types
                    .insert(declaration.name.clone(), sum_type.clone());

                // Bind each nullary variant as a value of the sum type, so a bare
                // `Red` resolves as an expression. (Variants with payloads are
                // resolved as constructor calls in `check_call`.) Nullary-ness is name
                // and arity only, identical in the parsed and resolved variant lists.
                for variant in variants {
                    if variant.fields.is_empty() {
                        self.env.define_constant(
                            variant.name.clone(),
                            sum_type.clone(),
                            declaration.span.clone(),
                        )?;
                    }
                }

                sum_type
            }
            TypeDefinition::Record { fields, methods } => {
                // The declaration itself, built once: every method's `it` binding and the
                // registered type are the same record, so they share one copy of it. Each
                // field's annotation is resolved (like a parameter's) so a field typed as a
                // user sum or record carries its real definition, not an empty placeholder.
                let record_fields = Rc::new(
                    fields
                        .iter()
                        .map(|(field_name, field_type)| {
                            (field_name.clone(), self.resolve_type(field_type))
                        })
                        .collect::<Vec<_>>(),
                );
                let method_names: Rc<Vec<String>> =
                    Rc::new(methods.iter().map(|m| m.name.clone()).collect());
                Type::Named {
                    name: declaration.name.clone(),
                    fields: record_fields,
                    methods: method_names,
                }
            }
        };

        // Register the type name in the environment BEFORE checking its methods, so a
        // method body may name its own type (constructing it, an operator returning it).
        // (Sum constructor lookup already went through `sum_types` above.)
        self.env.define(
            declaration.name.clone(),
            type_value.clone(),
            false,
            declaration.span.clone(),
        )?;

        // `it` binds to the type; operator members register on their operator's overload
        // set, every other member becomes a method dispatched by receiver type.
        self.check_type_methods(
            &declaration.name,
            &type_value,
            declaration.type_definition.methods(),
        )?;

        Ok(())
    }

    /// Type-check a type's methods (a record's members or a sum's `{ }` block). `self_type`
    /// is what `it` binds to — the record for a record, the whole sum value for a sum.
    /// An operator-symbol member (`==`, `+`, …) registers on its operator's overload set
    /// (`it` = left operand, the one explicit parameter = right); every other member — a
    /// named method or the render `` ` `` — becomes a receiver-dispatched method. Operator
    /// members register FIRST so a body may use the type's own operator.
    pub(super) fn check_type_methods(
        &mut self,
        type_name: &str,
        self_type: &Type,
        methods: &[crate::ast::MethodDeclaration],
    ) -> Result<(), TypeError> {
        for method in methods {
            if crate::ast::is_operator_symbol(&method.name) {
                self.register_operator_member(self_type, method)?;
            }
        }

        for method in methods {
            self.env.push_scope();
            let enclosing_declaration = self.enter_declaration();
            // A setter's receiver is mutable at every call site (owned by the enclosing
            // declaration, so a result aliasing it stays classified that way); an `=`
            // method's receiver is argument slot 0, its mutability the call site's.
            let is_setter = self
                .setter_methods
                .contains(&(type_name.to_string(), method.name.clone()));
            if is_setter {
                self.env.define_setter_receiver(
                    crate::ast::RECEIVER.to_string(),
                    self_type.clone(),
                    enclosing_declaration,
                    method.span.clone(),
                )?;
            } else {
                self.env.define_receiver(
                    self_type.clone(),
                    self.current_declaration,
                    method.span.clone(),
                )?;
            }

            // A method is dispatched on its receiver's type rather than called by name, so
            // it never receives a call site — its last parameter included. Every parameter
            // must be annotated: there is no `Num` default (same rule as an ordinary
            // definition's parameters — see `check_function_declaration`).
            let method_parameter_types: Vec<Type> = method
                .parameters
                .iter()
                .map(|p| {
                    p.type_annotation
                        .clone()
                        .ok_or_else(|| TypeError::UnannotatedParameter {
                            function: format!("{}.{}", type_name, method.name),
                            parameter: p.name.clone(),
                            span: p.span.clone(),
                        })
                })
                .collect::<Result<_, _>>()?;
            self.reject_unfillable_site_parameters(
                &format!("method `{}.{}`", type_name, method.name),
                &method.parameters,
                &method_parameter_types,
                false,
            )?;

            let mut resolved_parameter_types = Vec::with_capacity(method_parameter_types.len());
            for (slot, (parameter, raw_type)) in method
                .parameters
                .iter()
                .zip(&method_parameter_types)
                .enumerate()
            {
                // Resolve the annotation so a user-type parameter (`other :: Color`) carries
                // its fields/variants — field access and matching on it then resolve. The
                // type being defined is already registered (see `check_type_declaration`),
                // so a parameter naming it (an operator's right operand) resolves too.
                let parameter_type = self.resolve_type(raw_type);
                resolved_parameter_types.push(parameter_type.clone());
                // Slot 0 is the receiver; explicit parameters follow it, matching a
                // member call's argument list.
                self.env.define_parameter(
                    parameter.name.clone(),
                    parameter_type,
                    self.current_declaration,
                    slot + 1,
                    parameter.span.clone(),
                )?;
            }

            // Type-check the body, then resolve the method's result type: the annotation
            // when present, otherwise the inferred body type. Storing the *resolved* type
            // keeps call sites in agreement with codegen (an unannotated setter whose body
            // is a field write yields `$`, not Num). The annotation is resolved FIRST so an
            // otherwise-uninferable empty collection literal in the body can take its
            // element type from it (see `infer_expression_expecting`).
            let annotated_return_type = method.return_type.as_ref().map(|t| self.resolve_type(t));
            let body_type =
                self.infer_expression_expecting(&method.body, annotated_return_type.as_ref())?;
            let resolved_return_type = if let Some(resolved) = annotated_return_type {
                self.check_type_compatibility(&resolved, &body_type, &method.span)?;
                // A generic return annotation (`-> Result`) is refined to the inferred body
                // type, exactly as a top-level function's is — see
                // `check_function_declaration` for why.
                if resolved.contains_generic() {
                    body_type
                } else {
                    resolved
                }
            } else {
                body_type
            };

            // The render operator `` ` `` must render to `Text` and take only its implicit
            // `it` receiver (interpolation/`print` call it with no extra arguments).
            if method.name == "`" {
                if !method.parameters.is_empty() {
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

            // Classify the result's aliasing while the receiver and parameters are still
            // in scope: a member returning `it` (or anything holding it) is what makes a
            // call's result inherit the receiver's mutability at each call site.
            let result_aliasing =
                self.declaration_result_aliasing(&method.body, &resolved_return_type);

            // Which of a SETTER's own explicit parameters its body stores directly into a
            // field of `it` — computed here, still inside the body's scope, so a stored
            // parameter's own aliasing resolves. A setter call then requires exactly those
            // arguments to be `:=`-reachable (see `check_call`), the same store-crosses-
            // the-line rule a direct field write already enforces.
            if is_setter {
                let stored_parameter_slots = self.setter_stored_parameter_slots(&method.body);
                if !stored_parameter_slots.is_empty() {
                    self.setter_stored_parameters.insert(
                        (type_name.to_string(), method.name.clone()),
                        stored_parameter_slots,
                    );
                }
            }

            self.env.pop_scope();
            self.leave_declaration(enclosing_declaration);

            if crate::ast::is_operator_symbol(&method.name) {
                // An operator member dispatches through its overload set, so its
                // classification lives on the registered member.
                let mut operator_parameters = vec![self_type.clone()];
                operator_parameters.extend(resolved_parameter_types);
                self.set_overload_result_aliasing(
                    &method.name,
                    &operator_parameters,
                    result_aliasing,
                );
            } else if result_aliasing != ResultAliasing::default() {
                self.method_result_aliasing.insert(
                    (type_name.to_string(), method.name.clone()),
                    result_aliasing,
                );
            }

            // A method whose body never reads `it` needs no receiver VALUE at all — it may
            // be called on the bare type name (`Point.origin()`), the natural spelling for
            // a constructor. Operator members are excluded: they dispatch through the
            // overload set on `it` <op> `other`, never through a type-name receiver.
            if !crate::ast::is_operator_symbol(&method.name)
                && !body_references_receiver(&method.body)
            {
                self.static_methods
                    .insert((type_name.to_string(), method.name.clone()));
            }

            self.methods.insert(
                (type_name.to_string(), method.name.clone()),
                (
                    method.parameters.clone(),
                    resolved_return_type,
                    method.body.clone(),
                ),
            );
        }
        Ok(())
    }

    /// Apply the declared mutation contract to `type_name`'s methods: register the
    /// `:=`-declared ones as setters, then hold each `=`-declared one to its promise.
    ///
    /// Registration comes first so the verifier sees every sibling, which is what lets the
    /// transitive rule be a lookup: a method calling a `:=` sibling on `it` mutates by
    /// proxy, and every sibling's contract is known from its declaration.
    fn check_method_mutation_contracts(
        &mut self,
        type_name: &str,
        methods: &[crate::ast::MethodDeclaration],
    ) -> Result<(), TypeError> {
        for method in methods.iter().filter(|m| m.mutating) {
            self.setter_methods
                .insert((type_name.to_string(), method.name.clone()));
        }
        for method in methods.iter().filter(|m| !m.mutating) {
            // Point at the write itself: that is what broke the promise, and the method's
            // own span starts after the `=` the message tells the author to change.
            if let Some(span) = self.body_mutates_receiver(type_name, &method.body) {
                return Err(TypeError::MutatingMethodDeclaredImmutable {
                    type_name: type_name.to_string(),
                    method: method.name.clone(),
                    lambda_parameter_shadows_receiver: body_has_lambda_parameter_named_receiver(
                        &method.body,
                    ),
                    span,
                });
            }
        }
        Ok(())
    }

    /// Where does `expression` (a method body) mutate the receiver `it`, if anywhere?
    /// Yields the span of the first such sub-expression: a field write rooted at `it` (`it.field := …`, `it.a.b := …`) or a
    /// call to a `:=`-declared sibling applied to `it`.
    ///
    /// A write anywhere in the body counts — nested in a lambda, an array or record
    /// literal, a match arm, an argument list, or a locally declared function's body.
    /// Missing one would let an `=`-declared method mutate its receiver in silence. Both
    /// signals are node-local, so this is a flat predicate over the shared structural
    /// walk, the one place that has to know every expression form.
    pub(super) fn body_mutates_receiver(
        &self,
        type_name: &str,
        expression: &Expression,
    ) -> Option<Span> {
        match try_for_each_subexpression(expression, &mut |e| match self
            .node_mutates_receiver(type_name, e)
        {
            true => ControlFlow::Break(e.span().clone()),
            false => ControlFlow::Continue(()),
        }) {
            ControlFlow::Break(span) => Some(span),
            ControlFlow::Continue(()) => None,
        }
    }

    /// Is THIS expression itself a mutation of `it` — ignoring its sub-expressions, which
    /// the caller's walk visits separately?
    fn node_mutates_receiver(&self, type_name: &str, expression: &Expression) -> bool {
        match expression {
            Expression::FieldAssign { target, .. } | Expression::IndexAssign { target, .. } => {
                Self::field_path_root_name(target).as_deref() == Some(crate::ast::RECEIVER)
            }
            // `it.setter(...)` desugars to `setter(it, ...)`: a sibling setter applied to
            // `it` propagates "mutating" to the caller.
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                // The receiver test is free; the set probe allocates a key, so it goes
                // second — this runs on every call node in every method body.
                arguments.first().is_some_and(|recv| {
                    matches!(recv, Expression::Identifier { name, .. } if name == crate::ast::RECEIVER)
                }) && matches!(function.as_ref(), Expression::Identifier { name, .. }
                    if self.setter_methods.contains(&(type_name.to_string(), name.clone())))
            }
            _ => false,
        }
    }

    /// The name of the variable at the root of a field-access/index path, if any:
    /// `a.b.c` -> `Some("a")`, `a.b[i]` -> `Some("a")`. Returns `None` if the root isn't a
    /// plain ident.
    pub(super) fn field_path_root_name(target: &Expression) -> Option<String> {
        match target {
            Expression::FieldAccess { expression, .. } | Expression::Index { expression, .. } => {
                Self::field_path_root_name(expression)
            }
            Expression::Identifier { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Which of a setter's own explicit parameters (slot 1 = the first explicit
    /// parameter, the receiver being slot 0) its body stores directly into a field of
    /// `it` — every `it.path := value` in the body where `value`'s aliasing includes one
    /// of the setter's own parameters. Run while the method's scope is still pushed, the
    /// same window `declaration_result_aliasing` runs in, so a stored parameter's own
    /// aliasing (through a local, a container, or as itself) resolves.
    fn setter_stored_parameter_slots(&self, body: &Expression) -> std::collections::HashSet<usize> {
        let mut slots = std::collections::HashSet::new();
        let _ = try_for_each_subexpression(body, &mut |expression| {
            match expression {
                // A write reaches `it` however its target got there — a plain field chain
                // (`it.item := …`) or one hopping through an element read
                // (`it.items[i].sub := …`) — so this reads the target's resolved MUTABLE
                // WITNESS rather than walking its own path (`field_path_root_name` only
                // sees a chain of plain field accesses, missing an `Index` hop).
                Expression::FieldAssign { target, value, .. } => {
                    if let Expression::FieldAccess {
                        expression: base, ..
                    } = target.as_ref()
                        && self.value_aliasing(base).reaches_setter_receiver
                    {
                        self.record_stored_slot(value, &mut slots);
                    }
                }
                // A call to a stored-slot setter on an `it`-reachable receiver
                // (`it.inner.set(k)`) is the same store one call deeper: whichever of
                // ITS parameters `set` itself stores into `it` (already classified —
                // `set` type-checked before this setter, either an earlier-declared
                // type or an earlier sibling method; a later sibling calling forward is
                // already `UnknownMember` before this walk ever runs) makes the
                // MATCHING ARGUMENT here reach `it` too.
                Expression::Call {
                    function,
                    arguments,
                    member_call: true,
                    ..
                } => {
                    if let Expression::Identifier { name: method, .. } = function.as_ref()
                        && let Some(receiver) = arguments.first()
                        && self.value_aliasing(receiver).reaches_setter_receiver
                        && let Some(Type::Named {
                            name: type_name, ..
                        }) = self.type_table.get(receiver.span())
                        && let Some(nested_stored_slots) = self
                            .setter_stored_parameters
                            .get(&(type_name.clone(), method.clone()))
                    {
                        for (slot, argument) in arguments[1..].iter().enumerate() {
                            if nested_stored_slots.contains(&(slot + 1)) {
                                self.record_stored_slot(argument, &mut slots);
                            }
                        }
                    }
                }
                _ => {}
            }
            ControlFlow::<()>::Continue(())
        });
        slots
    }

    /// Which of THIS setter's own parameters `value`'s aliasing includes, recorded into
    /// `slots` — shared by a direct field write and a nested stored-slot setter call.
    fn record_stored_slot(&self, value: &Expression, slots: &mut std::collections::HashSet<usize>) {
        for (declaration, slot, _) in self.value_aliasing(value).parameters {
            if declaration == self.current_declaration {
                slots.insert(slot);
            }
        }
    }

    pub(super) fn check_variable_declaration(
        &mut self,
        declaration: &VariableDeclaration,
    ) -> Result<(), TypeError> {
        // Resolve the annotation FIRST (when present) so an otherwise-uninferable empty
        // collection literal on the right (`xs :: []Text = []`) can take its element type
        // from it — see `infer_expression_expecting`.
        let annotated_type = declaration
            .type_annotation
            .as_ref()
            .map(|t| self.resolve_type(t));
        let value_type =
            self.infer_expression_expecting(&declaration.value, annotated_type.as_ref())?;

        // If type annotation exists, check it matches
        let final_type = if let Some(annotated_type) = annotated_type {
            self.check_type_compatibility(&annotated_type, &value_type, &declaration.span)?;
            // A sum annotation (e.g. the generic `Result`) is satisfied by a more concrete
            // value of the same sum (`Ok(SomeRecord)`); keep the inferred type so its
            // specialized payloads survive the binding. Without this a later match on the
            // binding would unpack an aggregate payload at the generic fallback type.
            let specializes_sum = matches!(
                (&annotated_type, &value_type),
                (Type::Sum { name: a, .. }, Type::Sum { name: b, .. }) if a == b
            );
            if specializes_sum {
                value_type
            } else {
                annotated_type
            }
        } else {
            value_type
        };

        // Rebinding an `=` name is its own error, reported before any aliasing gate: the
        // fix is the binding operator, not the value.
        if declaration.mutable
            && self.env.get_type(&declaration.name).is_some()
            && !self.env.is_mutable(&declaration.name)
        {
            return Err(TypeError::ImmutableAssignment {
                name: declaration.name.clone(),
                span: declaration.span.clone(),
            });
        }

        // Deep immutability: a reference-typed value may not cross the `=`/`:=` line in
        // either direction. Binding it `:=` while an `=` binding (or a parameter, whose
        // argument belongs to the caller) still reaches it would make the frozen value
        // writable; binding it `=` while a `:=` binding reaches it would let writes
        // change the frozen value underneath. A fresh value binds either way.
        let value_aliasing = self.value_aliasing(&declaration.value);
        if declaration.mutable {
            if let Some((witness, parameter)) = value_aliasing.immutable_witness() {
                return Err(TypeError::MutableAliasOfImmutable {
                    name: declaration.name.clone(),
                    aliased: witness.to_string(),
                    parameter,
                    span: declaration.span.clone(),
                });
            }
        } else if let Some(witness) = value_aliasing.mutable_witness() {
            return Err(TypeError::ImmutableAliasOfMutable {
                name: declaration.name.clone(),
                aliased: witness.to_string(),
                span: declaration.span.clone(),
            });
        }

        let bound_value_is_callable = matches!(final_type, Type::Function { .. });

        if declaration.mutable {
            // `:=` — reassign if the name is already bound, otherwise a new mutable binding.
            if let Some(existing_type) = self.env.get_type(&declaration.name) {
                // Reassignment: the new value must match the binding's type.
                self.check_type_compatibility(&existing_type, &final_type, &declaration.span)?;
            } else {
                self.env.define_binding(
                    declaration.name.clone(),
                    final_type,
                    true,
                    self.current_declaration,
                    value_aliasing,
                    declaration.span.clone(),
                )?;
            }
        } else {
            // `=` — immutable binding; a same-scope duplicate is a DuplicateDefinition.
            self.env.define_binding(
                declaration.name.clone(),
                final_type,
                false,
                self.current_declaration,
                value_aliasing,
                declaration.span.clone(),
            )?;
        }

        // A binding whose value is itself callable (a closure) carries what CALLING it
        // later returns, classified from the bound expression the same way a named
        // function's own declaration is (`callable_result_aliasing`) — so `f = mk()`
        // then `x := f()` rejects `x` exactly as `x := mk()()` would, when the closure
        // `mk` returns one of `mk`'s own captured `=` locals. Set unconditionally
        // (mirroring `check_function_declaration`'s own unconditional set below) rather
        // than only when non-default: a `:=` REASSIGNMENT reuses the same symbol, so
        // skipping a fresh, default classification here would leave an earlier
        // assignment's non-default one stale on it.
        if bound_value_is_callable {
            let callable = self.callable_result_aliasing(&declaration.value);
            self.env.set_result_aliasing(&declaration.name, callable);
        }

        Ok(())
    }

    /// Reject a `Site` parameter that nothing could ever fill in, reported at the offending
    /// parameter.
    ///
    /// The compiler supplies a call site as the LAST argument of a call to a named
    /// top-level function, and nowhere else: a `Site` before another parameter could never
    /// be the omitted one, and a lambda, a nested declaration, or a record method is not
    /// called by name at all (a lambda and a capturing nested declaration are function
    /// VALUES, and a method dispatches on its receiver's type). `trailing_is_fillable` is
    /// the one thing that differs between those cases.
    fn reject_unfillable_site_parameters(
        &self,
        subject: &str,
        parameters: &[Parameter],
        parameter_types: &[Type],
        trailing_is_fillable: bool,
    ) -> Result<(), TypeError> {
        let unfillable = match trailing_is_fillable {
            true => parameter_types.len().saturating_sub(1),
            false => parameter_types.len(),
        };
        match parameters
            .iter()
            .zip(parameter_types)
            .take(unfillable)
            .find(|(_, ty)| crate::ast::is_site_type(ty))
        {
            Some((parameter, _)) => Err(TypeError::MisplacedSiteParameter {
                subject: subject.to_string(),
                span: parameter.span.clone(),
            }),
            None => Ok(()),
        }
    }

    /// Record each parameter's resolved type in the type oracle, keyed by the parameter's
    /// own span. A parameter that took its type from context has nothing written in the
    /// AST for codegen to read, so this side-table is where codegen recovers it — the same
    /// route it already takes for every inferred expression type.
    pub(super) fn record_parameter_types(&mut self, parameters: &[Parameter], types: &[Type]) {
        for (parameter, ty) in parameters.iter().zip(types) {
            self.type_table.insert(parameter.span.clone(), ty.clone());
        }
    }

    /// Resolve a definition's parameter types and record them in the oracle. A written
    /// annotation wins; a parameter without one takes the matching slot of `declared` — the
    /// function type the position or the binding states — and an annotation that disagrees
    /// with its slot is reported at that parameter. With no annotation and no slot there is
    /// nothing to infer from, and `missing` names the error for the kind of definition this
    /// is. Shared by every parameter list whose types can come from context.
    fn resolve_parameter_types(
        &mut self,
        parameters: &[Parameter],
        declared: Option<&[Type]>,
        missing: impl Fn(&Parameter) -> TypeError,
    ) -> Result<Vec<Type>, TypeError> {
        let types: Vec<Type> = parameters
            .iter()
            .enumerate()
            .map(|(i, p)| match (&p.type_annotation, declared) {
                (Some(annotation), slots) => {
                    let annotated = self.resolve_type(annotation);
                    if let Some(slots) = slots {
                        self.check_type_compatibility(
                            &self.resolve_type(&slots[i]),
                            &annotated,
                            &p.span,
                        )?;
                    }
                    Ok(annotated)
                }
                (None, Some(slots)) => Ok(self.resolve_type(&slots[i])),
                (None, None) => Err(missing(p)),
            })
            .collect::<Result<_, _>>()?;
        self.record_parameter_types(parameters, &types);
        Ok(types)
    }

    /// A function type on the binding has to describe the definition it sits on, or it is
    /// a lie about the function: it must have a slot per parameter, and — where a `->` is
    /// written beside it — the two must agree on the return type. (A parameter annotated
    /// against its slot is caught per parameter, in `resolve_parameter_types`.)
    fn check_binding_signature(&self, declaration: &FunctionDeclaration) -> Result<(), TypeError> {
        let Some(Type::Function {
            parameters,
            return_type,
        }) = &declaration.binding_type
        else {
            return Ok(());
        };
        if parameters.len() != declaration.parameters.len() {
            return Err(TypeError::SignatureArity {
                subject: format!("'{}'", declaration.name),
                expected: parameters.len(),
                got: declaration.parameters.len(),
                span: declaration.span.clone(),
            });
        }
        match &declaration.return_type {
            Some(arrow) => self.check_type_compatibility(
                &self.resolve_type(return_type),
                &self.resolve_type(arrow),
                &declaration.span,
            ),
            None => Ok(()),
        }
    }

    pub(super) fn check_function_declaration(
        &mut self,
        declaration: &FunctionDeclaration,
        nesting: Nesting,
    ) -> Result<(), TypeError> {
        // The inert core.io `print`/`eprint` placeholder is fully provided by the
        // compiler as a built-in overload; ignore its declaration entirely.
        if declaration.is_inert_corelib_placeholder() {
            return Ok(());
        }

        self.check_binding_signature(declaration)?;

        // Build function type from parameters and return type. A parameter takes its
        // written annotation, else the matching slot of a function type declared on the
        // binding itself. With neither it is a compile-time error: there is no `Num`
        // default, and nothing else here to infer from.
        let parameter_types = self.resolve_parameter_types(
            &declaration.parameters,
            declaration.declared_parameters(),
            |p| TypeError::UnannotatedParameter {
                function: declaration.name.clone(),
                parameter: p.name.clone(),
                span: p.span.clone(),
            },
        )?;

        // Only a top-level function's LAST parameter can receive a call site.
        self.reject_unfillable_site_parameters(
            &format!("function `{}`", declaration.name),
            &declaration.parameters,
            &parameter_types,
            nesting == Nesting::TopLevel,
        )?;

        let annotated_return = declaration
            .declared_return_type()
            .map(|t| self.resolve_type(t));

        // An overloaded member (operator-named, or one of 2+ same-named defs) is NOT
        // a single `env` binding — its signature already lives in the overload set
        // (registered in the pre-pass). We only type-check its body here, then refine
        // that member's return type from the inferred body when it wasn't annotated.
        let is_overloaded = self.overloaded_names.contains(&declaration.name);

        // For recursion support, the function needs to be callable from its own body —
        // but only when its return type is already KNOWN (annotated): a self-recursive
        // call needs to know what the call it's sitting inside of returns, and an
        // unannotated function's return type isn't known until that body is fully
        // checked. So an ANNOTATED function is defined in `env` before its body is
        // checked (enabling recursion), while an UNANNOTATED one is left undefined for
        // that window and marked `pending_return_type` instead — `check_call` reports a
        // clear error for a recursive call that resolves to it, rather than either
        // `UndefinedVariable` or (the historical bug) silently assuming `Num`.
        let previous_pending = self.pending_return_type.take();
        if !is_overloaded {
            match &annotated_return {
                Some(ret) => {
                    let func_type = Type::Function {
                        parameters: parameter_types.clone(),
                        return_type: Box::new(ret.clone()),
                    };
                    self.env.define(
                        declaration.name.clone(),
                        func_type,
                        false,
                        declaration.span.clone(),
                    )?;
                }
                None => {
                    self.pending_return_type =
                        Some((declaration.name.clone(), declaration.span.clone()));
                }
            }
        }

        // Push scope for body type checking
        self.env.push_scope();
        let enclosing_declaration = self.enter_declaration();

        // Add parameters to scope, each as its own argument slot: a parameter's value
        // belongs to the caller, so the body may not alias it mutably, and a result built
        // from it inherits the argument's mutability at each call site.
        for (slot, (parameter, parameter_type)) in declaration
            .parameters
            .iter()
            .zip(parameter_types.iter())
            .enumerate()
        {
            self.env.define_parameter(
                parameter.name.clone(),
                parameter_type.clone(),
                self.current_declaration,
                slot,
                parameter.span.clone(),
            )?;
        }

        // Check body and infer return type through the same contextual-typing helper a
        // call argument uses (`infer_argument`): a FUNCTION-typed return annotation types
        // a lambda body's otherwise-unannotated parameters — the third contextual-typing
        // position, after arguments and declared bindings — so
        // `adder = (n :: Num) -> (Num) -> Num => (x) => x + n` types `x` from the return;
        // any OTHER annotation is instead the expected type an otherwise-uninferable empty
        // collection literal body takes its element type from
        // (`f = () -> []Text => []`). Anything but a literal lambda or empty literal body
        // infers exactly as before either way.
        let target = match &annotated_return {
            Some(ret) => LambdaTarget::Declared(ret),
            None => LambdaTarget::None,
        };
        let body_type = self.infer_argument(&declaration.body, target)?;

        // Restore whatever the ENCLOSING declaration's pending state was — this function
        // may itself be nested inside another unannotated one still being checked.
        self.pending_return_type = previous_pending;

        // Classify the result's aliasing while the parameters are still in scope: a
        // function returning a parameter (however wrapped) makes each call's result
        // inherit that argument's mutability at the call site. A function returning a
        // CLOSURE (a function-typed body — `mk`, say, returning a lambda that captured
        // one of `mk`'s own `=` locals, or even one of `mk`'s OWN parameters) is
        // classified the other way: not what the returned closure VALUE aliases
        // (nothing — a function value isn't reference-typed), but what CALLING it later
        // aliases — computed one declaration deeper, at the returned lambda itself
        // (`callable_result_aliasing`), then re-bucketed as THIS declaration's own
        // (`reclassify_returned_closure`) exactly the way a directly-returned value's
        // aliasing already is, so a captured PARAMETER of `mk` becomes an argument slot
        // substituted at each of `mk`'s own call sites rather than a permanent witness.
        // Carried on this function's own binding so a caller who calls the result
        // inherits it (see `check_variable_declaration`).
        let result_aliasing = match &body_type {
            Type::Function { .. } => {
                self.reclassify_returned_closure(self.callable_result_aliasing(&declaration.body))
            }
            _ => self.declaration_result_aliasing(&declaration.body, &body_type),
        };

        self.env.pop_scope();
        self.leave_declaration(enclosing_declaration);

        // Verify the return type matches if annotated
        if let Some(annotated_type) = annotated_return {
            self.check_type_compatibility(&annotated_type, &body_type, &declaration.span)?;
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
                    parameters: parameter_types.clone(),
                    return_type: Box::new(body_type.clone()),
                };
                let _ = self.env.update_type(&declaration.name, refined);
            }
        } else if is_overloaded {
            // An overload member's return type is its annotation, never its inferred body
            // type. Adopting the body type here would make the member's signature depend
            // on where a call sits relative to the definition — a call above it would see
            // one type and a call below it another — which is precisely the order
            // dependence the annotation requirement removes. The omission is reported
            // instead, at the call or at the definition.
        } else {
            // Not annotated, not overloaded: `declaration.name` was left undefined for the
            // body-check window (see above), so define it now, for real, with the type its
            // body just proved — nothing before this point could have called it without
            // hitting `RecursiveFunctionNeedsReturnType`.
            let func_type = Type::Function {
                parameters: parameter_types.clone(),
                return_type: Box::new(body_type.clone()),
            };
            self.env.define(
                declaration.name.clone(),
                func_type,
                false,
                declaration.span.clone(),
            )?;
        }

        // Record the classification where calls resolve this definition: the overload
        // member for an overloaded name, the binding otherwise.
        match is_overloaded {
            true => self.set_overload_result_aliasing(
                &declaration.name,
                &parameter_types,
                result_aliasing,
            ),
            false => self
                .env
                .set_result_aliasing(&declaration.name, result_aliasing),
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
    /// Closures are MONOMORPHIC: parameters are concrete-typed and captured values are
    /// concrete. The language has no type variables, so there is nothing polymorphic to
    /// capture; generic closures + defunctionalization are deferred.
    ///
    /// Checked with no target type — the caller of this one has no signature to offer. A
    /// lambda in a position that DOES state one goes through
    /// [`Self::check_lambda_against`] instead.
    pub(super) fn check_lambda(
        &mut self,
        parameters: &[Parameter],
        return_type: Option<&Type>,
        body: &Expression,
    ) -> Result<Type, TypeError> {
        self.check_lambda_against(parameters, return_type, body, LambdaTarget::None)
    }

    /// Type-check a lambda against the type its position states — **contextual typing**.
    /// Where that is a function type of the same arity, each parameter the lambda leaves
    /// unannotated takes its type from the matching slot, so a higher-order call states the
    /// parameter types once, at the receiving definition. A written annotation always wins.
    ///
    /// Where the position states no usable type, an unannotated parameter is an error
    /// naming it — there is no silent `Num` — and the [`LambdaTarget`] is what the message
    /// says was missing.
    pub(super) fn check_lambda_against(
        &mut self,
        parameters: &[Parameter],
        return_type: Option<&Type>,
        body: &Expression,
        target: LambdaTarget<'_>,
    ) -> Result<Type, TypeError> {
        // Only a function type can type these parameters, and only slot for slot: a
        // different arity is a mismatch with the stated type, not a missing annotation.
        let slots = match target.stated() {
            Some(Type::Function {
                parameters: slots, ..
            }) => {
                if slots.len() != parameters.len() {
                    return Err(TypeError::SignatureArity {
                        subject: "this lambda".to_string(),
                        expected: slots.len(),
                        got: parameters.len(),
                        span: body.span().clone(),
                    });
                }
                Some(slots.as_slice())
            }
            _ => None,
        };

        let parameter_types =
            self.resolve_parameter_types(parameters, slots, |p| target.uninferable(p))?;

        // A lambda is a function VALUE, called through its binding rather than by name, so
        // no parameter of it — last included — can receive a call site.
        self.reject_unfillable_site_parameters("a lambda", parameters, &parameter_types, false)?;

        self.env.push_scope();
        let enclosing_declaration = self.enter_declaration();
        for (slot, (parameter, parameter_type)) in
            parameters.iter().zip(parameter_types.iter()).enumerate()
        {
            self.env.define_parameter(
                parameter.name.clone(),
                parameter_type.clone(),
                self.current_declaration,
                slot,
                parameter.span.clone(),
            )?;
        }
        // A FUNCTION-typed `-> Type` annotation types a lambda body contextually, the
        // same way a named function's return annotation does (see
        // `check_function_declaration`) — so a closure returned from a closure needs no
        // repeated parameter annotations.
        let annotated_return = return_type.map(|t| self.resolve_type(t));
        let body_type = match &annotated_return {
            Some(ret @ Type::Function { .. }) => {
                self.infer_argument(body, LambdaTarget::Declared(ret))?
            }
            _ => self.infer_expression(body)?,
        };

        // Classify the lambda's own captures while its scope is still pushed, exactly as
        // a named function's result is (`check_function_declaration`) — looked up later
        // wherever the lambda is called without going through a named binding
        // (`callable_result_aliasing`).
        self.record_lambda_result_aliasing(body, &body_type);

        self.env.pop_scope();
        self.leave_declaration(enclosing_declaration);

        // Honor an explicit `-> Type` annotation; otherwise the body's inferred type is
        // the return type.
        let ret = match annotated_return {
            Some(annotated) => {
                self.check_type_compatibility(&annotated, &body_type, body.span())?;
                annotated
            }
            None => body_type,
        };

        Ok(Type::Function {
            parameters: parameter_types,
            return_type: Box::new(ret),
        })
    }
}

/// Whether `body` reads the receiver `it` anywhere — a member call desugars `it.foo()`
/// into a `Call` whose first argument is the identifier `it`, so this plain identifier
/// search already covers a sibling call on the receiver too. Over-approximates like
/// [`body_has_lambda_parameter_named_receiver`]'s sibling checks: a lambda parameter that
/// shadows `it` still counts as a reference here, which only ever says "not static" more
/// often than strictly necessary — never the other way, which is the direction that would
/// let a wrongly-allowed static call reach a real `it` read.
pub(super) fn body_references_receiver(body: &Expression) -> bool {
    try_for_each_subexpression(body, &mut |e| match e {
        Expression::Identifier { name, .. } if name == crate::ast::RECEIVER => {
            ControlFlow::Break(())
        }
        _ => ControlFlow::Continue(()),
    })
    .is_break()
}

/// Whether `body` contains a lambda with a parameter named `it`. `it` is an ordinary
/// identifier, so such a parameter shadows the method receiver inside the lambda — and a
/// write through it is then reported as a receiver mutation. The flag lets the diagnostic
/// name the shadowing as the likely cause.
fn body_has_lambda_parameter_named_receiver(body: &Expression) -> bool {
    try_for_each_subexpression(body, &mut |e| match e {
        Expression::Lambda { parameters, .. }
            if parameters.iter().any(|p| p.name == crate::ast::RECEIVER) =>
        {
            ControlFlow::Break(())
        }
        _ => ControlFlow::Continue(()),
    })
    .is_break()
}
