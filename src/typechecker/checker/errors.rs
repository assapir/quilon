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
            | TypeError::DuplicateDefinition { span, .. }
            | TypeError::NoMatchingOverload { span, .. }
            | TypeError::AmbiguousOverload { span, .. }
            | TypeError::OverloadMissingAnnotation { span, .. }
            | TypeError::SiteIsImmutable { span, .. }
            | TypeError::MisplacedSiteParam { span, .. }
            | TypeError::OverloadCallBeforeDefinition { span, .. }
            | TypeError::UnannotatedOverloadCall { span, .. }
            | TypeError::UnannotatedOverloadMember { span, .. }
            | TypeError::ComparisonOverloadNotBool { span, .. }
            | TypeError::RefutableConstructorArg { span, .. }
            | TypeError::NonExhaustiveMatch { span }
            | TypeError::InvalidEntryPointSignature { span, .. }
            | TypeError::InvalidBuiltinArgument { span, .. }
            | TypeError::ComputedGlobalBinding { span, .. } => span,
        }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TypeError::UndefinedVariable { name, .. } => {
                write!(f, "Undefined variable '{}'", name)
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
            TypeError::OverloadMissingAnnotation { name, param, .. } => {
                write!(
                    f,
                    "Overloaded definition '{}' must annotate every parameter; '{}' has no type annotation",
                    name, param
                )
            }
            TypeError::SiteIsImmutable { field, .. } => {
                write!(
                    f,
                    "cannot write `{}`: a `Site` is read-only — a location is a value, not a variable",
                    field
                )
            }
            TypeError::MisplacedSiteParam { subject, .. } => {
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
            TypeError::UnannotatedOverloadCall { name, params, .. } => {
                write!(
                    f,
                    "cannot call '{}': its overload member ({}) has no return type annotation — annotate it, since exact dispatch needs the full signature",
                    name,
                    fmt_type_list(params)
                )
            }
            TypeError::UnannotatedOverloadMember { name, params, .. } => {
                write!(
                    f,
                    "overload member '{}' ({}) has no return type annotation — annotate it, since exact dispatch needs the full signature",
                    name,
                    fmt_type_list(params)
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
            TypeError::NonExhaustiveMatch { .. } => {
                write!(f, "Non-exhaustive pattern match")
            }
            TypeError::InvalidEntryPointSignature { got, .. } => {
                write!(
                    f,
                    "Entry point '^' has an unsupported signature ({}). Valid signatures: \
                     '()', '(args :: []Text)', '(args :: []Text, env :: [][]Text)' \
                     (or legacy '(argc :: Num, argv :: Num)').",
                    fmt_type_list(got)
                )
            }
            TypeError::InvalidBuiltinArgument { message, .. } => {
                write!(f, "{}", message)
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

/// Render a comma-separated parameter/argument type list (`Num, Text`).
pub(super) fn fmt_type_list(types: &[Type]) -> String {
    types.iter().map(type_label).collect::<Vec<_>>().join(", ")
}

/// Render candidate signatures for an overload diagnostic (`(Num, Num), (Text, Text)`).
pub(super) fn fmt_candidates(candidates: &[Vec<Type>]) -> String {
    candidates
        .iter()
        // A trailing `Site` is filled in by the compiler and can never be written at a call
        // site, so a candidate list must not ask for it — `assertEq(1)` reports the
        // candidates as `(Num, Num)`, not `(Num, Num, Site)`.
        .map(|params| format!("({})", fmt_type_list(crate::ast::visible_params(params))))
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::error::Error for TypeError {}
