//! `?`/`|` matching: checking each arm's pattern against the scrutinee, binding what it
//! names, and requiring a sum-typed match to cover every variant.
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

        if arms.is_empty() {
            return Err(TypeError::NonExhaustiveMatch { span: span.clone() });
        }

        // Check exhaustiveness for sum types
        if let Type::Sum { ref variants, .. } = expression_type {
            self.check_exhaustiveness(variants, arms, span)?;
        }

        // Check each arm's pattern against expression_type
        let mut result_type = None;

        for arm in arms {
            self.check_pattern(&arm.pattern, &expression_type)?;

            // Bind pattern variables and check body
            self.env.push_scope();
            self.bind_pattern_vars(&arm.pattern, &expression_type)?;

            let body_type = self.infer_expression(&arm.body)?;

            self.env.pop_scope();

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

        Ok(result_type.unwrap())
    }

    pub(super) fn check_exhaustiveness(
        &self,
        variants: &[crate::ast::SumVariant],
        arms: &[MatchArm],
        span: &Span,
    ) -> Result<(), TypeError> {
        // Collect all constructor patterns
        let mut covered_variants = std::collections::HashSet::new();
        let mut has_wildcard = false;

        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
                    has_wildcard = true;
                }
                Pattern::Constructor { name, .. } => {
                    covered_variants.insert(name.clone());
                }
                _ => {}
            }
        }

        // If we have a wildcard, we're exhaustive
        if has_wildcard {
            return Ok(());
        }

        // Check if all variants are covered
        for variant in variants {
            if !covered_variants.contains(&variant.name) {
                return Err(TypeError::NonExhaustiveMatch { span: span.clone() });
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
            Pattern::Wildcard { .. } => Ok(()), // Wildcard matches anything
            Pattern::Constructor {
                name,
                arguments,
                span,
            } => {
                // Check if the constructor matches the expected type
                // For now, accept all constructors - proper sum type checking would verify
                // that the constructor belongs to the expected sum type
                match expected_type {
                    Type::Sum { variants, .. } => {
                        // Find the variant with this constructor name
                        let variant = variants.iter().find(|v| v.name == *name);

                        if let Some(variant) = variant {
                            // Check that argument count matches
                            if variant.fields.len() != arguments.len() {
                                return Err(TypeError::WrongNumberOfArguments {
                                    expected: variant.fields.len(),
                                    got: arguments.len(),
                                    span: span.clone(),
                                });
                            }

                            // A payload sub-pattern must be IRREFUTABLE (a binding or
                            // `_`). Codegen dispatches on the constructor tag alone, so
                            // a refutable sub-pattern (`Ok(1)`, `Ok(Ok(x))`) would be
                            // silently ignored — the arm would match ANY payload of the
                            // variant, taking the wrong arm with no diagnostic. Reject
                            // it here until codegen tests payloads.
                            for pattern_arg in arguments {
                                match pattern_arg {
                                    Pattern::Identifier { .. } | Pattern::Wildcard { .. } => {}
                                    Pattern::Number { .. } | Pattern::Constructor { .. } => {
                                        return Err(TypeError::RefutableConstructorArg {
                                            constructor: name.clone(),
                                            span: pattern_arg.span().clone(),
                                        });
                                    }
                                }
                            }

                            Ok(())
                        } else {
                            // Constructor not found in sum type
                            Ok(()) // For now, accept it
                        }
                    }
                    _ => {
                        // Not a sum type, but we have a constructor pattern
                        // This is okay for now - we may be matching against
                        // a value that will be a sum type later
                        Ok(())
                    }
                }
            }
        }
    }

    pub(super) fn bind_pattern_vars(
        &mut self,
        pattern: &Pattern,
        type_: &Type,
    ) -> Result<(), TypeError> {
        match pattern {
            Pattern::Identifier { name, span } => {
                self.env
                    .define(name.clone(), type_.clone(), false, span.clone())?;
                Ok(())
            }
            Pattern::Constructor {
                name: constructor_name,
                arguments,
                ..
            } => {
                // For sum type constructors, bind arguments with their field types
                if let Type::Sum { variants, .. } = type_ {
                    // Find the variant that matches this constructor
                    if let Some(variant) = variants.iter().find(|v| &v.name == constructor_name) {
                        // Bind each argument with its corresponding field type
                        for (arg_pattern, field_type) in arguments.iter().zip(variant.fields.iter())
                        {
                            self.bind_pattern_vars(arg_pattern, field_type)?;
                        }
                    }
                } else {
                    // Not a sum type - fall back to binding with the same type
                    for arg in arguments {
                        self.bind_pattern_vars(arg, type_)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
