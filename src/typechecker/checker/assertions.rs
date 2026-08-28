//! Checking the provided assertions: `assert(actual, matcher)` and `expect(actual, matcher)`.
//!
//! Both are compiler-provided (see [`crate::ast::MATCHERS`]), so this is where a matcher's
//! shape and the types it can compare are settled — codegen then lowers the same form.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods run
//! against.

use super::*;

impl TypeChecker {
    /// Check `assert(actual, matcher)` / `expect(actual, matcher)`, yielding `$`.
    ///
    /// `expect` is rejected outside an `it` case in a `describe` block: it marks the running
    /// case failed, and the case's close is what tallies that. Outside a case nothing would
    /// report the failure, and outside a `describe` there is no run at all — the blocks
    /// are left out of everything but `quilon test`.
    pub(super) fn check_assertion(
        &mut self,
        name: &str,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<Type, TypeError> {
        if name == crate::ast::EXPECT && (self.test_depth == 0 || self.case_depth == 0) {
            return Err(TypeError::ExpectOutsideTest { span: span.clone() });
        }
        let [actual, matcher] = arguments else {
            return Err(TypeError::AssertionNeedsMatcher {
                name: name.to_string(),
                span: span.clone(),
            });
        };
        let actual_type = self.infer_expression(actual)?;
        self.check_matcher(name, &actual_type, matcher)?;
        Ok(Type::Unit)
    }

    /// Check one matcher against the type of the value it will be applied to. `not(matcher)`
    /// recurses on the same type, so a negation composes with any matcher.
    fn check_matcher(
        &mut self,
        assertion: &str,
        actual_type: &Type,
        matcher: &Expression,
    ) -> Result<(), TypeError> {
        let Expression::Call {
            function,
            arguments,
            span,
            ..
        } = matcher
        else {
            return Err(TypeError::AssertionNeedsMatcher {
                name: assertion.to_string(),
                span: matcher.span().clone(),
            });
        };
        let Expression::Identifier { name, .. } = function.as_ref() else {
            return Err(TypeError::AssertionNeedsMatcher {
                name: assertion.to_string(),
                span: matcher.span().clone(),
            });
        };
        if !crate::ast::is_matcher(name) {
            return Err(TypeError::AssertionNeedsMatcher {
                name: assertion.to_string(),
                span: matcher.span().clone(),
            });
        }
        let wanted = usize::from(name.as_str() != "isOk" && name.as_str() != "isNotOk");
        if arguments.len() != wanted {
            return Err(TypeError::MatcherArity {
                matcher: name.clone(),
                expected: wanted,
                got: arguments.len(),
                span: span.clone(),
            });
        }
        match name.as_str() {
            // Compared through `==`, so a user record or sum works exactly as far as its own
            // `==` member does.
            "equals" => {
                let expected_type = self.infer_expression(&arguments[0])?;
                self.check_type_compatibility(actual_type, &expected_type, span)?;
                self.require_one_representation(actual_type, &expected_type, span)?;
                self.require_equality(name, actual_type, span)
            }
            // A `Text` part of a `Text`, or one element of an array.
            "contains" => {
                let part_type = self.infer_expression(&arguments[0])?;
                match actual_type {
                    Type::Text => {
                        self.check_type_compatibility(&Type::Text, &part_type, span)?;
                        self.require_one_representation(&Type::Text, &part_type, span)
                    }
                    Type::Array(element) => {
                        self.check_type_compatibility(element, &part_type, span)?;
                        self.require_one_representation(element, &part_type, span)?;
                        self.require_equality(name, element, span)
                    }
                    other => Err(TypeError::MatcherTypeUnsupported {
                        matcher: name.clone(),
                        ty: Box::new(other.clone()),
                        span: span.clone(),
                    }),
                }
            }
            "not" => self.check_matcher(assertion, actual_type, &arguments[0]),
            // A `Result` — or any sum carrying the variant being asked about.
            matcher_name => {
                let asked_about = crate::ast::matcher_variant(matcher_name).ok_or_else(|| {
                    TypeError::AssertionNeedsMatcher {
                        name: assertion.to_string(),
                        span: span.clone(),
                    }
                })?;
                match actual_type {
                    Type::Sum { variants, .. }
                        if variants.iter().any(|variant| variant.name == asked_about) =>
                    {
                        Ok(())
                    }
                    other => Err(TypeError::MatcherTypeUnsupported {
                        matcher: name.clone(),
                        ty: Box::new(other.clone()),
                        span: span.clone(),
                    }),
                }
            }
        }
    }

    /// The two sides of a comparison must share one runtime REPRESENTATION. A `Generic` — a
    /// sum payload whose type is not yet concrete — is represented as a `Num`, while
    /// `types_compatible` treats it as a wildcard that pairs with anything; without this,
    /// `equals("x")` on a generic payload would type-check and then hand codegen an `f64` and
    /// a `Text` to compare.
    fn require_one_representation(
        &self,
        actual: &Type,
        expected: &Type,
        span: &Span,
    ) -> Result<(), TypeError> {
        let mismatched = |one: &Type, other: &Type| {
            matches!(one, Type::Generic { .. })
                && !matches!(other, Type::Generic { .. } | Type::Num)
        };
        match mismatched(actual, expected) || mismatched(expected, actual) {
            true => Err(TypeError::TypeMismatch {
                expected: Box::new(actual.clone()),
                got: Box::new(expected.clone()),
                span: span.clone(),
            }),
            false => Ok(()),
        }
    }

    /// `ty` must be comparable with `==` — the built-in member for a scalar, or the type's
    /// own `==` member. The matchers that compare values are exactly as capable as `==` is.
    fn require_equality(&self, matcher: &str, ty: &Type, span: &Span) -> Result<(), TypeError> {
        match self.has_exact_overload("==", &[ty.clone(), ty.clone()]) {
            true => Ok(()),
            false => Err(TypeError::MatcherTypeUnsupported {
                matcher: matcher.to_string(),
                ty: Box::new(ty.clone()),
                span: span.clone(),
            }),
        }
    }
}
