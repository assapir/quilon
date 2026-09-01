//! How a type error reports itself: the source span it points at, and the message the
//! CLI renders. The `TypeError` enum itself lives in `super`, beside the checker that
//! raises it.

use super::*;

impl TypeError {
    /// The source span this error refers to, for diagnostic rendering.
    pub fn span(&self) -> &Span {
        match self {
            TypeError::UndefinedVariable { span, .. }
            | TypeError::TypeMismatch { span, .. }
            | TypeError::NotAFunction { span, .. }
            | TypeError::WrongNumberOfArguments { span, .. }
            | TypeError::ImmutableAssignment { span, .. }
            | TypeError::ImmutableFieldWrite { span, .. }
            | TypeError::MutatingMethodOnImmutable { span, .. }
            | TypeError::MutatingMethodDeclaredImmutable { span, .. }
            | TypeError::DuplicateDefinition { span, .. }
            | TypeError::NoMatchingOverload { span, .. }
            | TypeError::AmbiguousOverload { span, .. }
            | TypeError::OverloadMissingAnnotation { span, .. }
            | TypeError::UnannotatedParameter { span, .. }
            | TypeError::SignatureArity { span, .. }
            | TypeError::UninferableLambdaParameter { span, .. }
            | TypeError::RecursiveFunctionNeedsReturnType { span, .. }
            | TypeError::SiteIsImmutable { span, .. }
            | TypeError::MisplacedSiteParameter { span, .. }
            | TypeError::OverloadCallBeforeDefinition { span, .. }
            | TypeError::UnannotatedOverloadCall { span, .. }
            | TypeError::UnannotatedOverloadMember { span, .. }
            | TypeError::ComparisonOverloadNotBool { span, .. }
            | TypeError::RefutableConstructorArg { span, .. }
            | TypeError::NonExhaustiveMatch { span, .. }
            | TypeError::UnknownConstructor { span, .. }
            | TypeError::ConstructorPatternOnNonSum { span, .. }
            | TypeError::InvalidEntryPointSignature { span, .. }
            | TypeError::InvalidBuiltinArgument { span, .. }
            | TypeError::ComputedGlobalBinding { span, .. }
            | TypeError::OperatorMustBeMember { span, .. }
            | TypeError::OperatorMemberArity { span, .. }
            | TypeError::AssertionNeedsMatcher { span, .. }
            | TypeError::ExpectOutsideTest { span }
            | TypeError::MatcherArity { span, .. }
            | TypeError::MatcherTypeUnsupported { span, .. }
            | TypeError::UnknownMember { span, .. }
            | TypeError::MethodCalledAsFunction { span, .. }
            | TypeError::NotRenderable { span, .. } => span,
        }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TypeError::UndefinedVariable { name, .. } => {
                write!(f, "Undefined variable '{}'", name)
            }
            TypeError::AssertionNeedsMatcher { name, .. } => {
                write!(
                    f,
                    "`{name}` takes the value and a matcher: `{name}(actual, equals(expected))`. \
                     The matchers are {}",
                    crate::ast::MATCHERS
                        .iter()
                        .map(|matcher| format!("`{matcher}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            TypeError::ExpectOutsideTest { .. } => {
                write!(
                    f,
                    "`{}` marks the running test case failed, so it only works inside an `{}` \
                     case in a `{}` block. Use `{}`, which reports and exits, everywhere else",
                    crate::ast::EXPECT,
                    crate::ast::display_name(crate::ast::TEST_CASE_MARKER),
                    crate::ast::display_name(crate::ast::TEST_BLOCK_MARKER),
                    crate::ast::ASSERT
                )
            }
            TypeError::MatcherArity {
                matcher,
                expected,
                got,
                ..
            } => {
                write!(f, "`{matcher}` takes {expected} argument(s), got {got}")
            }
            TypeError::MatcherTypeUnsupported { matcher, ty, .. } => {
                let label = type_label(ty);
                match matcher.as_str() {
                    "contains" => write!(
                        f,
                        "`contains` reads a `Text` or an array, and {label} is neither"
                    ),
                    "isOk" | "isNotOk" => write!(
                        f,
                        "`{matcher}` reads a `Result`, and {label} has no `{}` variant",
                        crate::ast::matcher_variant(matcher).unwrap_or_default()
                    ),
                    _ => write!(
                        f,
                        "`{matcher}` compares with `==`, which {label} has no member for"
                    ),
                }
            }
            TypeError::TypeMismatch { expected, got, .. } => {
                write!(f, "Type mismatch: expected {:?}, got {:?}", expected, got)
            }
            TypeError::NotAFunction { got, .. } => {
                write!(f, "Not a function: got {:?}", got)
            }
            TypeError::WrongNumberOfArguments { expected, got, .. } => {
                write!(
                    f,
                    "Wrong number of arguments: expected {}, got {}",
                    expected, got
                )
            }
            TypeError::ImmutableAssignment { name, .. } => {
                write!(f, "Cannot assign to immutable variable '{}'", name)
            }
            TypeError::ImmutableFieldWrite { name, .. } => {
                write!(
                    f,
                    "Cannot write to a field of immutable '{}'; bind it with ':=' to allow in-place mutation",
                    name
                )
            }
            TypeError::MutatingMethodDeclaredImmutable {
                type_name, method, ..
            } => {
                write!(
                    f,
                    "Method '{}.{}' mutates 'it' but is declared with '='; declare it with ':=' to allow in-place mutation",
                    type_name, method
                )
            }
            TypeError::MutatingMethodOnImmutable {
                method, receiver, ..
            } => {
                write!(
                    f,
                    "Cannot call mutating method '{}' on immutable '{}'; bind it with ':=' to allow in-place mutation",
                    method, receiver
                )
            }
            TypeError::DuplicateDefinition { name, .. } => {
                write!(f, "Duplicate definition of '{}'", name)
            }
            TypeError::NoMatchingOverload {
                name,
                arg_types,
                candidates,
                ..
            } => {
                write!(
                    f,
                    "No overload of '{}' matches argument types ({}). Candidates: {}",
                    name,
                    fmt_type_list(arg_types),
                    fmt_candidates(candidates),
                )
            }
            TypeError::AmbiguousOverload {
                name,
                arg_types,
                candidates,
                ..
            } => {
                write!(
                    f,
                    "Ambiguous call to '{}' with argument types ({}); multiple overloads match: {}",
                    name,
                    fmt_type_list(arg_types),
                    fmt_candidates(candidates),
                )
            }
            TypeError::OverloadMissingAnnotation {
                name, parameter, ..
            } => {
                write!(
                    f,
                    "Overloaded definition '{}' must annotate every parameter; '{}' has no type annotation",
                    name, parameter
                )
            }
            TypeError::UnannotatedParameter {
                function,
                parameter,
                ..
            } => {
                write!(
                    f,
                    "parameter '{}' of '{}' has no type: annotate it (its type cannot be inferred from context)",
                    parameter, function
                )
            }
            TypeError::SignatureArity {
                subject,
                expected,
                got,
                ..
            } => {
                write!(
                    f,
                    "{} takes {}, but the function type it must match takes {}",
                    subject,
                    fmt_parameter_count(*got),
                    expected
                )
            }
            TypeError::UninferableLambdaParameter {
                parameter,
                open_overload,
                ..
            } => {
                write!(
                    f,
                    "parameter '{parameter}' of this lambda has no type: annotate it — "
                )?;
                match open_overload {
                    Some(name) => write!(
                        f,
                        "the other arguments do not narrow '{name}' to a single overload, \
                         so what this position expects is not decided yet"
                    ),
                    None => write!(f, "nothing here states a function type to take it from"),
                }
            }
            TypeError::RecursiveFunctionNeedsReturnType { function, .. } => {
                write!(
                    f,
                    "recursive function '{function}' needs an annotated return type (`-> T`) \
                     — a call to itself needs to already know what it returns, and that isn't \
                     known until its body (which the call sits inside) is fully checked"
                )
            }
            TypeError::SiteIsImmutable { field, .. } => {
                write!(
                    f,
                    "cannot write `{}`: a `Site` is read-only — a location is a value, not a variable",
                    field
                )
            }
            TypeError::MisplacedSiteParameter { subject, .. } => {
                write!(
                    f,
                    "{} declares a `Site` parameter that nothing can fill in — the compiler supplies a call site only as the LAST parameter of a top-level function",
                    subject
                )
            }
            TypeError::OverloadCallBeforeDefinition { name, .. } => {
                write!(
                    f,
                    "cannot call '{}' before its definition — Quilon resolves names top to bottom; move the definition above this call",
                    name
                )
            }
            TypeError::UnannotatedOverloadCall {
                name, parameters, ..
            } => {
                write!(
                    f,
                    "cannot call '{}': its overload member ({}) has no return type annotation — annotate it, since exact dispatch needs the full signature",
                    name,
                    fmt_type_list(parameters)
                )
            }
            TypeError::UnannotatedOverloadMember {
                name, parameters, ..
            } => {
                write!(
                    f,
                    "overload member '{}' ({}) has no return type annotation — annotate it, since exact dispatch needs the full signature",
                    name,
                    fmt_type_list(parameters)
                )
            }
            TypeError::ComparisonOverloadNotBool { operator, got, .. } => {
                write!(
                    f,
                    "comparison operator '{}' overload must return Bool, found {}",
                    operator,
                    type_label(got)
                )
            }
            TypeError::RefutableConstructorArg { constructor, .. } => {
                write!(
                    f,
                    "Unsupported pattern: an argument of '{}(…)' must be a binding or '_' \
                     — a literal or nested constructor here would silently match ANY \
                     payload. Bind the payload and compare it in the arm body instead.",
                    constructor
                )
            }
            TypeError::NonExhaustiveMatch {
                scrutinee, missing, ..
            } => match missing.is_empty() {
                true => write!(
                    f,
                    "this match on {} is not exhaustive — add a '_' arm for the values no arm \
                     lists",
                    type_label(scrutinee)
                ),
                false => write!(
                    f,
                    "this match on {} is not exhaustive — no arm covers {}. Add the missing \
                     arms, or a '_' arm",
                    type_label(scrutinee),
                    fmt_name_list(missing)
                ),
            },
            TypeError::UnknownConstructor {
                constructor,
                sum,
                known,
                ..
            } => {
                write!(
                    f,
                    "'{constructor}' is not a variant of '{sum}' — its variants are {}",
                    fmt_name_list(known)
                )
            }
            TypeError::ConstructorPatternOnNonSum {
                constructor, got, ..
            } => match **got {
                // An un-specialized payload (the `Result` slot nothing pinned to a concrete
                // type) has no variants to dispatch on either, but saying it "has no
                // variants" would describe the wrong problem: the type is missing, not
                // variant-less.
                Type::Generic { .. } => write!(
                    f,
                    "'{constructor}' is a sum-type variant pattern, and the type of this \
                     match's value is not known here — annotate it, or match it where its \
                     type is concrete",
                ),
                _ => write!(
                    f,
                    "'{constructor}' is a sum-type variant pattern, and this match is on {}, \
                     which has no variants — match the value itself instead: a literal, a \
                     binding, or '_'",
                    type_label(got)
                ),
            },
            TypeError::InvalidEntryPointSignature { got, .. } => {
                write!(
                    f,
                    "Entry point '^' has an unsupported signature ({}). Valid signatures: \
                     '()', '(args :: []Text)', '(args :: []Text, env :: [|Text => Text|])'.",
                    fmt_type_list(got)
                )
            }
            TypeError::InvalidBuiltinArgument { message, .. } => {
                write!(f, "{}", message)
            }
            TypeError::OperatorMustBeMember { operator, .. } => {
                write!(
                    f,
                    "operator '{operator}' cannot be defined at the top level — define it as a member of the record or sum type it operates on, where 'it' is the left operand (e.g. inside the type's '{{ }}')",
                )
            }
            TypeError::NotRenderable { name, got, .. } => {
                write!(
                    f,
                    "'{name}' renders its argument through the type's '`' member, and {} has none",
                    crate::ast::type_label(got),
                )
            }
            TypeError::OperatorMemberArity { operator, got, .. } => {
                write!(
                    f,
                    "operator member '{operator}' takes exactly one parameter (the right operand; 'it' is the left operand), but has {got}",
                )
            }
            TypeError::UnknownMember {
                type_name,
                member,
                in_scope,
                receiver,
                more_arguments,
                ..
            } => {
                write!(f, "'{type_name}' has no member '{member}'")?;
                let shown = receiver.as_deref().unwrap_or("receiver");
                let rest = match more_arguments {
                    true => ", ...",
                    false => "",
                };
                // An output built-in is the likely intent behind `c.print()`; say where it
                // lives and how to reach it, both read off the builtin's own full name.
                if let Some((module, _)) = crate::ast::RENDERABLE_BUILTINS
                    .iter()
                    .find(|builtin| crate::ast::display_name(builtin.name) == member)
                    .and_then(|builtin| builtin.name.rsplit_once('.'))
                {
                    let binding = crate::ast::display_name(module);
                    return write!(
                        f,
                        ". A value prints through `{module}`'s '{member}' — call it as \
                         '{binding}.{member}({shown}{rest})' (under `<< {module}`)"
                    );
                }
                if !in_scope {
                    return Ok(());
                }
                // There IS a function of that name, so say why it did not answer the call —
                // in terms of what was written, and with the call that would reach it.
                write!(
                    f,
                    ". There is a '{member}' in scope, but '{shown}.{member}(...)' only \
                     looks on {type_name} — call it as '{member}({shown}{rest})'"
                )
            }
            TypeError::MethodCalledAsFunction {
                type_name,
                member,
                receiver,
                more_arguments,
                ..
            } => {
                let receiver = receiver.as_deref().unwrap_or("receiver");
                let (rest, rest_only) = match more_arguments {
                    true => (", ...", "..."),
                    false => ("", ""),
                };
                write!(
                    f,
                    "no function '{member}' in scope — '{member}' is a member of \
                     {type_name}, which '{member}({receiver}{rest})' does not look on. \
                     Call it as '{receiver}.{member}({rest_only})'",
                )
            }
            TypeError::ComputedGlobalBinding { name, .. } => {
                write!(
                    f,
                    "top-level '{name}' has to be computed, and nothing runs before '^' to \
                     compute it. A top-level binding may hold a Num, Bool or $ literal, or a \
                     function; anything else (a call, an operator, an array, a record, Text) \
                     belongs inside a function — move it into '^' or the function that uses it."
                )
            }
        }
    }
}

/// Render a comma-separated quoted name list (`'Red', 'Green'`), for the variants a
/// diagnostic names.
pub(super) fn fmt_name_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a parameter count as English (`1 parameter`, `2 parameters`).
fn fmt_parameter_count(count: usize) -> String {
    match count {
        1 => "1 parameter".to_string(),
        n => format!("{n} parameters"),
    }
}

/// Render a comma-separated parameter/argument type list (`Num, Text`).
pub(super) fn fmt_type_list(types: &[Type]) -> String {
    types.iter().map(type_label).collect::<Vec<_>>().join(", ")
}

/// Render candidate signatures for an overload diagnostic (`(Num, Num), (Text, Text)`).
pub(super) fn fmt_candidates(candidates: &[Vec<Type>]) -> String {
    candidates
        .iter()
        // A trailing `Site` is filled in by the compiler and can never be written at a call
        // site, so a candidate list must not ask for it — `failAt(message, site)` reports
        // its candidate as `(Text)`, not `(Text, Site)`.
        .map(|parameters| {
            format!(
                "({})",
                fmt_type_list(crate::ast::visible_parameters(parameters))
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::error::Error for TypeError {}
