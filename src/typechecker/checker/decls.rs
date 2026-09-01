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
                        self.env.define(
                            variant.name.clone(),
                            sum_type.clone(),
                            false,
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
            self.env.define(
                crate::ast::RECEIVER.to_string(),
                self_type.clone(),
                false,
                method.span.clone(),
            )?;

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

            for (parameter, raw_type) in method.parameters.iter().zip(&method_parameter_types) {
                // Resolve the annotation so a user-type parameter (`other :: Color`) carries
                // its fields/variants — field access and matching on it then resolve. The
                // type being defined is already registered (see `check_type_declaration`),
                // so a parameter naming it (an operator's right operand) resolves too.
                let parameter_type = self.resolve_type(raw_type);
                self.env.define(
                    parameter.name.clone(),
                    parameter_type,
                    false,
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

            self.env.pop_scope();

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
            Expression::FieldAssign { target, .. } => {
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

    /// The name of the variable at the root of a field-access path, if any:
    /// `a.b.c` -> `Some("a")`. Returns `None` if the root isn't a plain ident.
    pub(super) fn field_path_root_name(target: &Expression) -> Option<String> {
        match target {
            Expression::FieldAccess { expression, .. } => Self::field_path_root_name(expression),
            Expression::Identifier { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// If a mutation rooted at `receiver` would write through an *immutable*
    /// binding, return that binding's name; otherwise `None`. A `:=`-bound
    /// receiver and the method receiver `it` (whose mutability is enforced at the
    /// outer call site) are both allowed. Shared by the field-write and
    /// setter-call mutability gates so they can never diverge.
    pub(super) fn immutable_mutation_root(&self, receiver: &Expression) -> Option<String> {
        let name = Self::field_path_root_name(receiver)?;
        if name != crate::ast::RECEIVER && !self.env.is_mutable(&name) {
            Some(name)
        } else {
            None
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

        if declaration.mutable {
            // `:=` — reassign if the name is already bound, otherwise a new mutable binding.
            if let Some(existing_type) = self.env.get_type(&declaration.name) {
                if !self.env.is_mutable(&declaration.name) {
                    return Err(TypeError::ImmutableAssignment {
                        name: declaration.name.clone(),
                        span: declaration.span.clone(),
                    });
                }
                // Reassignment: the new value must match the binding's type.
                self.check_type_compatibility(&existing_type, &final_type, &declaration.span)?;
                Ok(())
            } else {
                self.env.define(
                    declaration.name.clone(),
                    final_type,
                    true,
                    declaration.span.clone(),
                )
            }
        } else {
            // `=` — immutable binding; a same-scope duplicate is a DuplicateDefinition.
            self.env.define(
                declaration.name.clone(),
                final_type,
                false,
                declaration.span.clone(),
            )
        }
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

        // Add parameters to scope
        for (parameter, parameter_type) in declaration.parameters.iter().zip(parameter_types.iter())
        {
            self.env.define(
                parameter.name.clone(),
                parameter_type.clone(),
                false,
                parameter.span.clone(),
            )?;
        }

        // Check body and infer return type. The annotation (when present) is the expected
        // type, so an otherwise-uninferable empty collection literal directly in the body
        // (`f = () -> []Text => []`) can take its element type from it.
        let body_type =
            self.infer_expression_expecting(&declaration.body, annotated_return.as_ref())?;

        // Restore whatever the ENCLOSING declaration's pending state was — this function
        // may itself be nested inside another unannotated one still being checked.
        self.pending_return_type = previous_pending;

        self.env.pop_scope();

        // Returning a function value is deferred: a function may TAKE a function as a
        // parameter, but handing one back across the call boundary is not supported yet.
        // Checked on both the annotation and the inferred body so neither slips through.
        let returns_function = matches!(body_type, Type::Function { .. })
            || matches!(annotated_return, Some(Type::Function { .. }));
        if returns_function {
            return Err(TypeError::UnsupportedFunctionReturn {
                function: declaration.name.clone(),
                span: declaration.span.clone(),
            });
        }

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
                parameters: parameter_types,
                return_type: Box::new(body_type.clone()),
            };
            self.env.define(
                declaration.name.clone(),
                func_type,
                false,
                declaration.span.clone(),
            )?;
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
        for (parameter, parameter_type) in parameters.iter().zip(parameter_types.iter()) {
            self.env.define(
                parameter.name.clone(),
                parameter_type.clone(),
                false,
                parameter.span.clone(),
            )?;
        }
        let body_type = self.infer_expression(body)?;
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
            parameters: parameter_types,
            return_type: Box::new(ret),
        })
    }
}
