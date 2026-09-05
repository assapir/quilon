//! How a type error reports itself: the source span it points at, its code, the message
//! the CLI renders, and the labels and help that go with it. The `TypeError` enum itself
//! lives in `super`, beside the checker that raises it.

use super::*;
use crate::diagnostic::{Code, Diagnostic};

impl TypeError {
    /// The registry code this error reports under.
    pub fn code(&self) -> Code {
        match self {
            TypeError::UndefinedVariable { .. } => Code::UndefinedVariable,
            TypeError::TypeMismatch { .. } => Code::TypeMismatch,
            TypeError::NotAFunction { .. } => Code::NotAFunction,
            TypeError::WrongNumberOfArguments { .. } => Code::WrongNumberOfArguments,
            TypeError::ImmutableAssignment { .. } => Code::ImmutableAssignment,
            TypeError::ImmutableFieldWrite { .. } => Code::ImmutableFieldWrite,
            TypeError::MutableAliasOfImmutable { .. } => Code::MutableAliasOfImmutable,
            TypeError::ImmutableAliasOfMutable { .. } => Code::ImmutableAliasOfMutable,
            TypeError::MutableStoreOfImmutable { .. } => Code::MutableStoreOfImmutable,
            TypeError::MutatingMethodOnImmutable { .. } => Code::MutatingMethodOnImmutable,
            TypeError::MutatingMethodDeclaredImmutable { .. } => {
                Code::MutatingMethodDeclaredImmutable
            }
            TypeError::DuplicateDefinition { .. } => Code::DuplicateDefinition,
            TypeError::NoMatchingOverload { .. } => Code::NoMatchingOverload,
            TypeError::AmbiguousOverload { .. } => Code::AmbiguousOverload,
            TypeError::OverloadMissingAnnotation { .. } => Code::OverloadMissingAnnotation,
            TypeError::UnannotatedParameter { .. } => Code::UnannotatedParameter,
            TypeError::SignatureArity { .. } => Code::SignatureArity,
            TypeError::UninferableLambdaParameter { .. } => Code::UninferableLambdaParameter,
            TypeError::RecursiveFunctionNeedsReturnType { .. } => {
                Code::RecursiveFunctionNeedsReturnType
            }
            TypeError::SiteIsImmutable { .. } => Code::SiteIsImmutable,
            TypeError::MisplacedSiteParameter { .. } => Code::MisplacedSiteParameter,
            TypeError::OverloadCallBeforeDefinition { .. } => Code::OverloadCallBeforeDefinition,
            TypeError::UnannotatedOverloadCall { .. }
            | TypeError::UnannotatedOverloadMember { .. } => Code::UnannotatedOverloadMember,
            TypeError::ComparisonOverloadNotBool { .. } => Code::ComparisonOverloadNotBool,
            TypeError::RefutableConstructorArg { .. } => Code::RefutableConstructorArg,
            TypeError::NonExhaustiveMatch { .. } => Code::NonExhaustiveMatch,
            TypeError::UnknownConstructor { .. } => Code::UnknownConstructor,
            TypeError::ConstructorPatternOnNonSum { .. } => Code::ConstructorPatternOnNonSum,
            TypeError::InvalidEntryPointSignature { .. } => Code::InvalidEntryPointSignature,
            TypeError::InvalidBuiltinArgument { .. } => Code::InvalidBuiltinArgument,
            TypeError::ComputedGlobalBinding { .. } => Code::ComputedGlobalBinding,
            TypeError::OperatorMustBeMember { .. } => Code::OperatorMustBeMember,
            TypeError::OperatorMemberArity { .. } => Code::OperatorMemberArity,
            TypeError::AssertionNeedsMatcher { .. } => Code::AssertionNeedsMatcher,
            TypeError::ExpectOutsideTest { .. } => Code::ExpectOutsideTest,
            TypeError::MatcherArity { .. } => Code::MatcherArity,
            TypeError::MatcherTypeUnsupported { .. } => Code::MatcherTypeUnsupported,
            TypeError::UnknownMember { .. } => Code::UnknownMember,
            TypeError::MethodCalledAsFunction { .. } => Code::MethodCalledAsFunction,
            TypeError::NotRenderable { .. } => Code::NotRenderable,
            TypeError::StaticCallNeedsReceiverValue { .. } => Code::StaticCallNeedsReceiverValue,
            TypeError::ReservedName { .. } => Code::ReservedName,
        }
    }

    /// The error for binding `name`, if the language reserves it (see `ast::reserved_for`)
    /// — the one check every place a user-written name is bound runs.
    pub(super) fn reserved_name(name: &str, span: &Span) -> Option<TypeError> {
        crate::ast::reserved_for(name).map(|reserved_for| TypeError::ReservedName {
            name: name.to_string(),
            reserved_for,
            span: span.clone(),
        })
    }

    /// The error as a diagnostic: its code, message and span, plus the labels and help
    /// the variant has to offer.
    pub fn diagnostic(&self) -> Diagnostic {
        let diagnostic = Diagnostic::at(self.code(), self.span(), self.to_string());
        match self {
            TypeError::NoMatchingOverload {
                name,
                arg_types,
                arg_spans,
                candidates,
                ..
            } => {
                let mut diagnostic = Diagnostic::new(self.code(), self.to_string());
                // Each operand labelled with its type, when the spans are the operands'
                // own (an operator's are; a call's arguments too).
                if arg_spans.len() == arg_types.len() && !arg_spans.is_empty() {
                    for (span, ty) in arg_spans.iter().zip(arg_types) {
                        diagnostic = diagnostic.label(span, Some(type_label(ty)));
                    }
                } else {
                    diagnostic = diagnostic.label(self.span(), None);
                }
                let help = match (name.as_str(), arg_types.as_slice()) {
                    ("+", [Type::Num, Type::Text]) | ("+", [Type::Text, Type::Num]) => {
                        "to join a number and text, interpolate: \"`n`x\"".to_string()
                    }
                    _ => format!("the members of `{name}` are {}", fmt_candidates(candidates)),
                };
                diagnostic.help(help)
            }
            TypeError::OverloadCallBeforeDefinition { name, .. } => diagnostic.help(format!(
                "names resolve top to bottom: move the definition of `{name}` above this call"
            )),
            TypeError::ImmutableAssignment { name, .. }
            | TypeError::ImmutableFieldWrite { name, .. } => {
                diagnostic.help(format!("bind it with `:=` to allow writes: `{name} := …`"))
            }
            TypeError::MutatingMethodOnImmutable { receiver, .. } => {
                diagnostic.help(format!("bind it with `:=`: `{receiver} := …`"))
            }
            TypeError::MutatingMethodDeclaredImmutable { method, .. } => {
                diagnostic.help(format!("declare it with `:=`: `{method} := …`"))
            }
            TypeError::RecursiveFunctionNeedsReturnType { function, .. } => diagnostic.help(
                format!("annotate the return type: `{function} = (…) -> T => < … >`"),
            ),
            TypeError::NonExhaustiveMatch { missing, .. } => match missing.is_empty() {
                true => diagnostic.help("add a `_` arm for the values no arm lists"),
                false => diagnostic.help(format!(
                    "add an arm for {}, or a `_` arm",
                    fmt_name_list(missing)
                )),
            },
            TypeError::UnknownMember {
                member,
                in_scope,
                receiver,
                more_arguments,
                ..
            } => {
                let shown = receiver.as_deref().unwrap_or("receiver");
                let rest = match more_arguments {
                    true => ", ...",
                    false => "",
                };
                match output_builtin_module(member) {
                    Some(module) => diagnostic.help(format!(
                        "call it as `{}.{member}({shown}{rest})` under `<< {module}`",
                        crate::ast::display_name(module)
                    )),
                    None if *in_scope => {
                        diagnostic.help(format!("call it as `{member}({shown}{rest})`"))
                    }
                    None => diagnostic,
                }
            }
            TypeError::MethodCalledAsFunction {
                member,
                receiver,
                more_arguments,
                ..
            } => {
                let receiver = receiver.as_deref().unwrap_or("receiver");
                let rest = match more_arguments {
                    true => "...",
                    false => "",
                };
                diagnostic.help(format!("call it as `{receiver}.{member}({rest})`"))
            }
            TypeError::ExpectOutsideTest { .. } => diagnostic.help(format!(
                "use `{}`, which reports and exits, outside a test case",
                crate::ast::ASSERT
            )),
            TypeError::ComputedGlobalBinding { .. } => {
                diagnostic.help("move the computation into `^` or the function that uses it")
            }
            TypeError::OperatorMustBeMember { operator, .. } => diagnostic.help(format!(
                "define it inside the type's `{{ }}`, where `it` is the left operand: \
                 `{operator} = (other) => < … >`"
            )),
            TypeError::StaticCallNeedsReceiverValue { method, .. } => {
                diagnostic.help(format!("call it on a value: `x.{method}()`"))
            }
            TypeError::ReservedName { name, .. } => diagnostic.help(format!(
                "pick another name; a record field or method may still be called `{name}`"
            )),
            _ => diagnostic,
        }
    }

    /// The source span this error refers to, for diagnostic rendering.
    pub fn span(&self) -> &Span {
        match self {
            TypeError::UndefinedVariable { span, .. }
            | TypeError::TypeMismatch { span, .. }
            | TypeError::NotAFunction { span, .. }
            | TypeError::WrongNumberOfArguments { span, .. }
            | TypeError::ImmutableAssignment { span, .. }
            | TypeError::ImmutableFieldWrite { span, .. }
            | TypeError::MutableAliasOfImmutable { span, .. }
            | TypeError::ImmutableAliasOfMutable { span, .. }
            | TypeError::MutableStoreOfImmutable { span, .. }
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
            | TypeError::NotRenderable { span, .. }
            | TypeError::StaticCallNeedsReceiverValue { span, .. }
            | TypeError::ReservedName { span, .. } => span,
        }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TypeError::UndefinedVariable { name, .. } => {
                write!(f, "`{name}` is not defined")
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
                write!(
                    f,
                    "type mismatch: expected {}, got {}",
                    type_label(expected),
                    type_label(got)
                )
            }
            TypeError::NotAFunction { got, .. } => {
                write!(f, "{} is not a function", type_label(got))
            }
            TypeError::WrongNumberOfArguments { expected, got, .. } => {
                write!(
                    f,
                    "wrong number of arguments: expected {}, got {}",
                    expected, got
                )
            }
            TypeError::ImmutableAssignment { name, .. } => {
                write!(f, "cannot assign to `{name}`, which is bound with `=`")
            }
            TypeError::ImmutableFieldWrite { name, .. } => {
                write!(
                    f,
                    "cannot write to a field of `{name}`, which is bound with `=`"
                )
            }
            TypeError::MutableAliasOfImmutable {
                name,
                aliased,
                parameter,
                ..
            } => match parameter {
                false => write!(
                    f,
                    "cannot bind '{name}' with ':=': its value is '{aliased}''s, and \
                     '{aliased}' is immutable — a value bound with '=' stays immutable \
                     through every alias. Bind '{name}' with '=', or build a fresh value",
                ),
                true if aliased == crate::ast::RECEIVER => write!(
                    f,
                    "cannot bind '{name}' with ':=': its value is the receiver 'it''s, \
                     whose mutability belongs to the call site — a '='-declared method \
                     cannot make its receiver's value mutable. Bind '{name}' with '=', \
                     or build a fresh value",
                ),
                true => write!(
                    f,
                    "cannot bind '{name}' with ':=': its value is parameter '{aliased}''s, \
                     whose argument belongs to the caller and may be '='-bound — a \
                     parameter's value cannot be made mutable. Bind '{name}' with '=', or \
                     build a fresh value",
                ),
            },
            TypeError::ImmutableAliasOfMutable { name, aliased, .. } => {
                write!(
                    f,
                    "cannot bind '{name}' with '=': its value is '{aliased}''s, and \
                     '{aliased}' is mutable (':=') — writes through '{aliased}' would \
                     change '{name}' underneath. Bind '{name}' with ':=', or build a \
                     fresh value",
                )
            }
            TypeError::MutableStoreOfImmutable {
                aliased, parameter, ..
            } => match parameter {
                false => write!(
                    f,
                    "cannot store this value where a ':=' binding already reaches it: it \
                     is '{aliased}''s, and '{aliased}' is immutable — a value bound with \
                     '=' stays immutable through every alias. Store a fresh value, or a \
                     value reached only through ':=' bindings",
                ),
                true => write!(
                    f,
                    "cannot store this value where a ':=' binding already reaches it: it \
                     is parameter '{aliased}''s, whose argument belongs to the caller and \
                     may be '='-bound. Store a fresh value, or a value reached only \
                     through ':=' bindings",
                ),
            },
            TypeError::MutatingMethodDeclaredImmutable {
                type_name,
                method,
                lambda_parameter_shadows_receiver,
                ..
            } => {
                write!(
                    f,
                    "method `{type_name}.{method}` mutates `it` but is declared with `=`"
                )?;
                if *lambda_parameter_shadows_receiver {
                    write!(
                        f,
                        ". A lambda in this body names a parameter 'it', which shadows \
                         the receiver — if the write targets the lambda's own value, \
                         rename that parameter"
                    )?;
                }
                Ok(())
            }
            TypeError::MutatingMethodOnImmutable {
                method, receiver, ..
            } => {
                write!(
                    f,
                    "cannot call mutating method `{method}` on `{receiver}`, which is bound \
                     with `=`"
                )
            }
            TypeError::DuplicateDefinition { name, .. } => {
                write!(f, "duplicate definition of `{name}`")
            }
            TypeError::NoMatchingOverload {
                name, arg_types, ..
            } => {
                write!(
                    f,
                    "no overload of `{name}` takes ({})",
                    fmt_type_list(arg_types),
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
                write!(f, "`{name}` is called before its definition")
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
                    "this match on {} is not exhaustive",
                    type_label(scrutinee)
                ),
                false => write!(
                    f,
                    "this match on {} is not exhaustive — no arm covers {}",
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
                    "operator `{operator}` cannot be defined at the top level — an operator \
                     is a member of the record or sum type it operates on",
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
                ..
            } => {
                write!(f, "{type_name} has no member `{member}`")?;
                // An output built-in is the likely intent behind `c.print()`; say where it
                // lives. There being a function of that name is worth a word too: the
                // member form only looks on the type, so the function never answers it.
                if let Some(module) = output_builtin_module(member) {
                    return write!(f, " — a value prints through `{module}`'s `{member}`");
                }
                if *in_scope {
                    write!(
                        f,
                        " — there is a `{member}` in scope, but `.{member}(...)` only looks \
                         on {type_name}"
                    )?;
                }
                Ok(())
            }
            TypeError::MethodCalledAsFunction {
                type_name,
                member,
                receiver,
                more_arguments,
                ..
            } => {
                let receiver = receiver.as_deref().unwrap_or("receiver");
                let rest = match more_arguments {
                    true => ", ...",
                    false => "",
                };
                write!(
                    f,
                    "no function `{member}` in scope — `{member}` is a member of \
                     {type_name}, which `{member}({receiver}{rest})` does not look on",
                )
            }
            TypeError::ComputedGlobalBinding { name, .. } => {
                write!(
                    f,
                    "top-level `{name}` has to be computed, and nothing runs before `^` to \
                     compute it — a top-level binding holds a Num, Bool or $ literal, or a \
                     function"
                )
            }
            TypeError::StaticCallNeedsReceiverValue {
                method, type_name, ..
            } => {
                write!(
                    f,
                    "`{method}` reads `it`, so `{type_name}.{method}()` needs a value of \
                     {type_name} — there is none"
                )
            }
            TypeError::ReservedName {
                name, reserved_for, ..
            } => {
                write!(f, "`{name}` is reserved for {reserved_for}")
            }
        }
    }
}

/// The module an output built-in (`print`/`eprint`/`write`) named `member` lives in — the
/// likely intent behind `value.print()`.
fn output_builtin_module(member: &str) -> Option<&'static str> {
    crate::ast::RENDERABLE_BUILTINS
        .iter()
        .find(|builtin| crate::ast::display_name(builtin.name) == member)
        .and_then(|builtin| builtin.name.rsplit_once('.'))
        .map(|(module, _)| module)
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
