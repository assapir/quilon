//! `?`/`|` matching: checking each arm's pattern against the scrutinee, binding what it
//! names, and requiring every match to be exhaustive.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;

impl TypeChecker {
    pub(super) fn check_match(
        &mut self,
        expression: &Expression,
        arms: &[MatchArm],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let expression_type = self.infer_expression(expression)?;

        // Recorded here, where the environment has already resolved shadowing and aliases.
        if let Expression::Identifier {
            name,
            span: scrutinee_span,
        } = expression
            && is_unspecialized_result(&expression_type)
            && let Some((declaration_body, parameter_index)) = self
                .env
                .lookup(name)
                .and_then(|symbol| symbol.result_parameter.clone())
        {
            use crate::ast::{NOT_OK, OK};

            let mut ok_binding = None;
            let mut not_ok_binding = None;
            for arm in arms {
                if let Pattern::Constructor {
                    name: constructor,
                    arguments,
                    ..
                } = &arm.pattern
                    && let [
                        Pattern::Identifier {
                            name: binding_name,
                            span: binding_span,
                        },
                    ] = arguments.as_slice()
                {
                    let binding = (binding_name.clone(), binding_span.clone(), arm.body.clone());
                    match constructor.as_str() {
                        OK => ok_binding = Some(binding),
                        NOT_OK => not_ok_binding = Some(binding),
                        _ => {}
                    }
                }
            }
            self.result_scrutinees
                .entry(declaration_body)
                .or_default()
                .push(ResultScrutinee {
                    parameter_index,
                    scrutinee_span: scrutinee_span.clone(),
                    ok_binding,
                    not_ok_binding,
                });
        }

        // The parser rejects an armless match, but the checker is a library entry point and
        // an AST can be built by hand — so answer with the diagnostic rather than falling
        // through to a match that yields nothing.
        if arms.is_empty() {
            return Err(TypeError::NonExhaustiveMatch {
                scrutinee: Box::new(expression_type),
                missing: Vec::new(),
                mutable_binding: false,
                span: span.clone(),
            });
        }

        // Each pattern first, then coverage: a pattern that cannot match this scrutinee at
        // all is the more specific complaint, and reporting "not exhaustive" over it would
        // send the reader to add an arm rather than fix the one they wrote.
        for arm in arms {
            self.check_pattern(&arm.pattern, &expression_type)?;
        }

        // A bare `=` binding scrutinee may already be proven down to fewer variants than
        // its sum declares (see `docs/types/sum-types.md`'s narrowing section) — a `:=`
        // binding never is (`Environment::narrowed_variants` answers `None` for it, the
        // same answer an unnarrowed `=` binding gets, so the two share the path below; the
        // mutability is only needed for the exhaustiveness error's help line).
        let scrutinee_binding = match expression {
            Expression::Identifier { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let narrowed = scrutinee_binding.and_then(|name| self.env.narrowed_variants(name).cloned());
        let mutable_binding = scrutinee_binding.is_some_and(|name| self.env.is_changeable(name));

        if let (Some(binding), Some(narrowed)) = (scrutinee_binding, &narrowed) {
            self.check_dead_arms(binding, narrowed, arms)?;
        }
        self.check_exhaustiveness(
            &expression_type,
            arms,
            narrowed.as_ref(),
            mutable_binding,
            span,
        )?;

        let mut result_type = None;

        // What a pattern binds — the whole scrutinee or a payload it carries — is part of
        // the scrutinee's value, so every binding inherits the scrutinee's aliasing: a
        // payload record matched out of a parameter's sum still counts as that
        // parameter's value.
        let scrutinee_aliasing = self.value_aliasing(expression);

        // The match's own aliasing (the union of its arms') is computed here, arm by
        // arm, while each arm's bindings are still in scope — `value_aliasing` caches it
        // under the match's span, since a later walk could no longer resolve them.
        let mut arms_aliasing = ValueAliasing::default();

        for arm in arms {
            // Confined to this arm: a fact an arm's own body proves (about the scrutinee
            // or anything else) does not survive past it, the same as a branch's does not
            // (see `Environment::snapshot_narrowing`).
            let narrowing_before = self.env.snapshot_narrowing();

            // Bind pattern variables and check body
            self.env.push_scope();
            self.bind_pattern_vars(&arm.pattern, &expression_type, &scrutinee_aliasing)?;

            // Inside `| Variant(p) => …`, a bare `=` scrutinee is known to be `Variant` —
            // one of this step's three narrowing sources (see `docs/types/sum-types.md`).
            if let (Some(binding), Pattern::Constructor { name, .. }) =
                (scrutinee_binding, &arm.pattern)
            {
                self.env
                    .narrow(binding, [name.clone()].into_iter().collect());
            }

            let body_type = self.infer_expression(&arm.body)?;
            arms_aliasing.merge(self.value_aliasing(&arm.body));

            self.env.pop_scope();
            self.env.restore_narrowing(narrowing_before);

            // All arms must agree (compatibly). Prefer the most concrete arm type as the
            // result: an arm binding an un-specialized payload (`Generic`, e.g. a
            // never-constructed `NotOk(e) => e`) must not make the whole match's result
            // type generic when another arm yields a concrete type — codegen needs a
            // concrete result type to size the match value. So when the running result is
            // `Generic` and this arm is concrete, upgrade to the concrete type.
            match result_type.take() {
                Some(expected_type) => {
                    self.check_type_compatibility(&expected_type, &body_type, &arm.span)?;
                    // A wholly-generic running result (e.g. a leading `NotOk(e) => e`
                    // arm) is replaced by this arm's type; otherwise merge so a
                    // `Result`-valued match keeps the concrete payload from whichever
                    // arm specialized each variant (`merge_types` prefers concrete
                    // per slot and leaves a non-sum type unchanged).
                    let next = if matches!(expected_type, Type::Generic { .. }) {
                        body_type
                    } else {
                        Self::merge_types(expected_type, &body_type)
                    };
                    result_type = Some(next);
                }
                None => result_type = Some(body_type),
            }
        }

        self.match_aliasing.insert(span.clone(), arms_aliasing);

        Ok(result_type.expect("an armless match was rejected above"))
    }

    /// Every match must be total. A sum-typed scrutinee is covered arm by arm — one per
    /// variant the scrutinee can still be (every variant, unless `narrowed` says fewer),
    /// or a catch-all. Any other scrutinee has no enumerable set of values, so a catch-all
    /// (`_`, or a binding) is the only way to cover it: without one, a value no arm lists
    /// would fall off the end of the match with no value to yield.
    pub(super) fn check_exhaustiveness(
        &self,
        scrutinee: &Type,
        arms: &[MatchArm],
        narrowed: Option<&std::collections::BTreeSet<String>>,
        mutable_binding: bool,
        span: &Span,
    ) -> Result<(), TypeError> {
        if arms.iter().any(|arm| arm.pattern.is_irrefutable()) {
            return Ok(());
        }

        let not_covered = |missing: Vec<String>| TypeError::NonExhaustiveMatch {
            scrutinee: Box::new(scrutinee.clone()),
            missing,
            mutable_binding,
            span: span.clone(),
        };

        let Type::Sum { variants, .. } = scrutinee else {
            return Err(not_covered(Vec::new()));
        };
        let covered: std::collections::HashSet<&str> = arms
            .iter()
            .filter_map(|arm| match &arm.pattern {
                Pattern::Constructor { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let required = variants
            .iter()
            .filter(|variant| narrowed.is_none_or(|set| set.contains(&variant.name)));
        let missing: Vec<String> = required
            .filter(|variant| !covered.contains(variant.name.as_str()))
            .map(|variant| variant.name.clone())
            .collect();

        match missing.is_empty() {
            true => Ok(()),
            false => Err(not_covered(missing)),
        }
    }

    /// A match arm that can only cover a variant the narrowing already ruled out — a
    /// constructor pattern naming a variant outside `narrowed`, or a catch-all (`_`, or a
    /// binding) when every variant it would otherwise reach is already ruled out too. Runs
    /// only when the scrutinee IS narrowed (a strict subset of its sum), so an ordinary,
    /// unnarrowed match is entirely unaffected.
    fn check_dead_arms(
        &self,
        binding: &str,
        narrowed: &std::collections::BTreeSet<String>,
        arms: &[MatchArm],
    ) -> Result<(), TypeError> {
        let covered: std::collections::HashSet<&str> = arms
            .iter()
            .filter_map(|arm| match &arm.pattern {
                Pattern::Constructor { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let dead = |variant: Option<String>, span: &Span| TypeError::UnreachableMatchArm {
            binding: binding.to_string(),
            variant,
            known: narrowed.iter().cloned().collect(),
            span: span.clone(),
        };
        for arm in arms {
            match &arm.pattern {
                Pattern::Constructor { name, span, .. } if !narrowed.contains(name) => {
                    return Err(dead(Some(name.clone()), span));
                }
                Pattern::Wildcard { span } | Pattern::Identifier { span, .. }
                    if narrowed
                        .iter()
                        .all(|variant| covered.contains(variant.as_str())) =>
                {
                    return Err(dead(None, span));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn check_pattern(
        &self,
        pattern: &Pattern,
        expected_type: &Type,
    ) -> Result<(), TypeError> {
        match pattern {
            Pattern::Identifier { .. } => Ok(()), // Any type can bind to ident
            Pattern::Number { .. } => {
                self.check_type_compatibility(&Type::Num, expected_type, pattern.span())
            }
            Pattern::Text { .. } => {
                self.check_type_compatibility(&Type::Text, expected_type, pattern.span())
            }
            Pattern::Wildcard { .. } => Ok(()), // Wildcard matches anything
            Pattern::Constructor {
                name,
                arguments,
                span,
            } => {
                // A constructor pattern names a variant of the scrutinee's sum type. Both
                // ways of getting that wrong — a scrutinee with no variants at all, and a
                // name none of them has — are rejected here: codegen dispatches on a tag it
                // looks up by name, so anything reaching it without a tag dies at run time.
                let Type::Sum {
                    name: sum,
                    variants,
                } = expected_type
                else {
                    return Err(TypeError::ConstructorPatternOnNonSum {
                        constructor: name.clone(),
                        got: Box::new(expected_type.clone()),
                        span: span.clone(),
                    });
                };
                let Some(variant) = variants.iter().find(|v| v.name == *name) else {
                    return Err(TypeError::UnknownConstructor {
                        constructor: name.clone(),
                        sum: sum.clone(),
                        known: variants.iter().map(|v| v.name.clone()).collect(),
                        span: span.clone(),
                    });
                };

                if variant.fields.len() != arguments.len() {
                    return Err(TypeError::WrongNumberOfArguments {
                        expected: variant.fields.len(),
                        got: arguments.len(),
                        span: span.clone(),
                    });
                }

                // A payload sub-pattern must be IRREFUTABLE (a binding or `_`). Codegen
                // dispatches on the constructor tag alone, so a refutable sub-pattern
                // (`Ok(1)`, `Ok(Ok(x))`) would be silently ignored — the arm would match
                // ANY payload of the variant, taking the wrong arm with no diagnostic.
                // Reject it here until codegen tests payloads.
                for pattern_arg in arguments {
                    if !pattern_arg.is_irrefutable() {
                        return Err(TypeError::RefutableConstructorArg {
                            constructor: name.clone(),
                            span: pattern_arg.span().clone(),
                        });
                    }
                }

                Ok(())
            }
        }
    }

    pub(super) fn bind_pattern_vars(
        &mut self,
        pattern: &Pattern,
        type_: &Type,
        scrutinee_aliasing: &ValueAliasing,
    ) -> Result<(), TypeError> {
        match pattern {
            Pattern::Identifier { name, span } => {
                self.env.define_binding(
                    name.clone(),
                    type_.clone(),
                    false,
                    self.current_declaration,
                    scrutinee_aliasing.clone(),
                    span.clone(),
                )?;
                Ok(())
            }
            Pattern::Constructor {
                name: constructor_name,
                arguments,
                ..
            } => {
                // Each payload sub-pattern binds at its variant's field type. `check_pattern`
                // has already established that the scrutinee is that sum and that the
                // constructor is one of its variants, so anything else binds nothing.
                if let Type::Sum { variants, .. } = type_
                    && let Some(variant) = variants.iter().find(|v| &v.name == constructor_name)
                {
                    for (arg_pattern, field_type) in arguments.iter().zip(variant.fields.iter()) {
                        let resolved_field_type = self.resolve_payload_type(field_type);
                        self.bind_pattern_vars(
                            arg_pattern,
                            &resolved_field_type,
                            scrutinee_aliasing,
                        )?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
