// Type checker implementation

use crate::ast::type_label;
use crate::ast::{BinaryOperator, UnaryOperator};
use crate::ast::{
    Expression, FunctionDeclaration, InterpolationPart, Item, MatchArm, Parameter, Pattern,
    Program, Type, VariableDeclaration,
};
use crate::lexer::Span;

// The checker's methods live in child modules — one per checking area — as further
// `impl TypeChecker` blocks. Children of this file rather than siblings under
// `typechecker`, so the state declared below stays private to the checker: a child can
// reach its ancestor's private items, a sibling could not.
mod assertions;
mod calls;
mod decls;
mod env;
mod errors;
mod exprs;
mod overloads;
mod patterns;
mod sums;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use sums::result_of;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeError {
    UndefinedVariable {
        name: String,
        span: Span,
    },
    TypeMismatch {
        // Boxed: keeps `TypeError` small so `Result<_, TypeError>` stays cheap to
        // pass by value (clippy::result_large_err).
        expected: Box<Type>,
        got: Box<Type>,
        span: Span,
    },
    NotAFunction {
        got: Type,
        span: Span,
    },
    WrongNumberOfArguments {
        expected: usize,
        got: usize,
        span: Span,
    },
    ImmutableAssignment {
        name: String,
        span: Span,
    },
    /// `obj.field := v` where `obj`'s root binding is immutable (`=`-bound).
    ImmutableFieldWrite {
        name: String,
        span: Span,
    },
    /// Calling a mutating (setter) method on an immutable (`=`-bound) receiver.
    MutatingMethodOnImmutable {
        method: String,
        receiver: String,
        span: Span,
    },
    /// `assert`/`expect` called with anything but a value and one of the provided matchers.
    AssertionNeedsMatcher {
        name: String,
        span: Span,
    },
    /// `expect` outside a `describe` block, where there is no reporter to record into.
    ExpectOutsideTest {
        span: Span,
    },
    /// A matcher given the wrong number of arguments.
    MatcherArity {
        matcher: String,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// A matcher applied to a type it cannot inspect — no `==` member to compare with, or
    /// not the shape the matcher reads.
    MatcherTypeUnsupported {
        matcher: String,
        ty: Box<Type>,
        span: Span,
    },
    /// An `=`-declared method whose body mutates `it`, breaking the promise its binding
    /// operator makes.
    MutatingMethodDeclaredImmutable {
        type_name: String,
        method: String,
        span: Span,
    },
    DuplicateDefinition {
        name: String,
        span: Span,
    },
    /// No overload of `name` accepts the given argument types (exact-match dispatch,
    /// no implicit coercion). Lists the available candidate signatures.
    NoMatchingOverload {
        name: String,
        arg_types: Vec<Type>,
        candidates: Vec<Vec<Type>>,
        span: Span,
    },
    /// More than one overload of `name` matches the given argument types. (With
    /// exact-match dispatch this means two overloads share a parameter-type list —
    /// a duplicate definition.) Lists the colliding candidate signatures.
    AmbiguousOverload {
        name: String,
        arg_types: Vec<Type>,
        candidates: Vec<Vec<Type>>,
        span: Span,
    },
    /// An overloaded definition (operator-named, or one of several same-named defs)
    /// left a parameter unannotated. Exact-type dispatch needs every member's
    /// parameter types spelled out.
    OverloadMissingAnnotation {
        name: String,
        parameter: String,
        span: Span,
    },
    /// A function parameter with no type annotation. A parameter type is no longer assumed
    /// to be `Num`: it must be written down. (A lambda passed to a built-in collection
    /// method is the exception — its parameter type comes from the element type.)
    UnannotatedParameter {
        function: String,
        parameter: String,
        span: Span,
    },
    /// A function whose result is itself a function value. Taking a function as a parameter
    /// works, but returning one across the call boundary is deferred, so it is rejected
    /// rather than miscompiled.
    UnsupportedFunctionReturn {
        function: String,
        span: Span,
    },
    /// A write to a field of a `Site`. The type is read-only as a whole: a location is a
    /// value, not a variable. It has to be — a compiler-filled call site is one shared
    /// read-only constant, and records alias, so a write through any binding of one would be
    /// a write to that constant.
    SiteIsImmutable {
        field: String,
        span: Span,
    },
    /// A `Site` parameter (the built-in call-site record the compiler fills in) that could
    /// never be filled: one declared before the last parameter, or on a lambda or method,
    /// neither of which is called by name. `subject` names what declared it.
    MisplacedSiteParameter {
        subject: String,
        span: Span,
    },
    /// A call to an overload set every member of which is defined *below* the call.
    /// Names resolve top to bottom, so the definition is not in scope yet.
    OverloadCallBeforeDefinition {
        name: String,
        span: Span,
    },
    /// A call resolved to an overload member whose definition omitted its return type
    /// annotation, so the call's result type is unknown. Anchored at the call — the
    /// place the missing annotation actually stops the program.
    UnannotatedOverloadCall {
        name: String,
        parameters: Vec<Type>,
        span: Span,
    },
    /// An overload member omitted its return type annotation and nothing calls it, so
    /// there is no call site to blame. Anchored at the definition.
    UnannotatedOverloadMember {
        name: String,
        parameters: Vec<Type>,
        span: Span,
    },
    /// A comparison/equality operator overload (`== != < <= > >=`) declared a non-`Bool`
    /// return type. These operators are predicates and must yield `Bool`.
    ComparisonOverloadNotBool {
        operator: String,
        got: Box<Type>,
        span: Span,
    },
    /// A constructor pattern's argument was itself refutable (a literal or a nested
    /// constructor, e.g. `Ok(1)` or `Ok(Ok(x))`). Codegen dispatches on the constructor
    /// tag alone and would silently ignore the sub-pattern — the arm would match ANY
    /// payload — so this is rejected until payload tests are implemented.
    RefutableConstructorArg {
        constructor: String,
        span: Span,
    },
    NonExhaustiveMatch {
        span: Span,
    },
    /// The `^` entry point declared an unsupported parameter signature. The only
    /// accepted forms are `()`, `(args :: []Text)`, `(args :: []Text, env :: [|Text => Text|])`,
    /// and the legacy `(argc :: Num, argv :: Num)`. Rejected here (not in codegen) so
    /// `quilon check` and `quilon run`/`build` all report the same clear diagnostic.
    InvalidEntryPointSignature {
        got: Vec<Type>,
        span: Span,
    },
    /// A built-in method call had a statically-invalid argument that isn't a plain type
    /// mismatch (e.g. `Text.replace`'s literal `count` being `<= 0`). Carries a ready-made
    /// message describing the problem and the fix.
    InvalidBuiltinArgument {
        message: String,
        span: Span,
    },
    /// A top-level binding's value has to be computed, which there is nowhere to do: a
    /// module-level binding becomes a global whose initializer must already be a constant,
    /// and nothing runs before `^` to fill one in. Only a `Num`/`Bool`/`$` literal or a
    /// function value qualifies. Rejected here (not in codegen) so `quilon check` and
    /// `quilon run`/`build` agree — reaching codegen produced an internal builder error
    /// (`UnsetPosition`), or, when the value was a call, a module whose instructions had
    /// been appended to whatever function was emitted last.
    ComputedGlobalBinding {
        name: String,
        span: Span,
    },
    /// A top-level definition was named by an operator symbol. Operator overloading now
    /// lives inside a type: an operator is a member of the record or sum type it operates
    /// on, with `it` as the left operand.
    OperatorMustBeMember {
        operator: String,
        span: Span,
    },
    /// An operator member had the wrong number of explicit parameters. A binary operator
    /// member takes exactly one (the right operand); `it` is the left operand.
    OperatorMemberArity {
        operator: String,
        got: usize,
        span: Span,
    },
}

/// Exact-type match for overload dispatch (no implicit coercion). Built-in scalars
/// match by identity; a user type matches by NAME (so a `Named`/`Sum` annotation and
/// the inferred instance line up regardless of carried fields); a `Generic` payload
/// slot (only Result's `Ok(T)`/`NotOk(E)`) matches anything, preserving the existing
/// generic-Result behavior.
pub(crate) fn types_match(parameter: &Type, arg: &Type) -> bool {
    match (parameter, arg) {
        // `Generic` is a not-yet-concrete type — only a sum payload binding (the `T` in
        // `Ok(T)`) produces one, since concrete sum-payload typing is a deferred 0.9
        // feature. For overload dispatch a `Generic` resolves as `Num`, the canonical
        // working payload (numeric payloads are sound end-to-end), so `Ok(x) => x * 2`
        // dispatches `*` to its `(Num, Num)` member. This means an overloaded call on a
        // generic value resolves DETERMINISTICALLY to the Num member rather than
        // matching every member (a spurious ambiguity) — and it never wildcard-matches a
        // user record/sum type. (A Text/Bool payload routed through an overload thus
        // picks the Num member — the documented non-numeric-payload limitation, not a
        // new behavior; true concrete-payload dispatch awaits that feature.)
        (Type::Generic { .. }, other) | (other, Type::Generic { .. }) => {
            matches!(other, Type::Num | Type::Generic { .. })
        }
        (Type::Num, Type::Num)
        | (Type::Text, Type::Text)
        | (Type::Bool, Type::Bool)
        | (Type::Unit, Type::Unit) => true,
        (Type::Array(a), Type::Array(b)) => types_match(a, b),
        (Type::Map(ka, va), Type::Map(kb, vb)) => types_match(ka, kb) && types_match(va, vb),
        (Type::Set(a), Type::Set(b)) => types_match(a, b),
        // User record / sum types are identified by name.
        (
            Type::Named { name: a, .. } | Type::Sum { name: a, .. },
            Type::Named { name: b, .. } | Type::Sum { name: b, .. },
        ) => a == b,
        _ => false,
    }
}

#[derive(Debug, Clone)]
// `pub` so it doesn't leak through the public `Environment::lookup` signature.
pub struct Symbol {
    type_: Type,
    mutable: bool,
}

#[derive(Debug, Clone)]
pub struct Environment {
    scopes: Vec<HashMap<String, Symbol>>,
}

/// A method's signature and body: (parameters, return type, body expression).
type MethodDef = (Vec<Parameter>, Option<Type>, Expression);

/// The **type oracle**: a side-table mapping each expression's source `Span` to the
/// `Type` the checker inferred for it. Produced by `check_program` and consumed by
/// codegen so that READ sites (array indexing, record-field access, match-arm results)
/// recover the *declared* element / field / result type instead of guessing `f64` from
/// a runtime LLVM value. Spans are unique per expression (every AST node carries its
/// own source span), so they make a stable, AST-agnostic key. See the consumer-side
/// wrapper `codegen::TypeOracle`.
pub type TypeTable = std::collections::HashMap<Span, Type>;

/// One member of an overload set: an exact parameter-type list and the result type.
/// Both named functions and operators (keyed by their symbol, e.g. `"+"`) live in the
/// same registry, and the compiler-lowered defaults (`+` on Num/Text, the comparisons,
/// `print`, …) are registered as ordinary members beside the user's own — the standard
/// operators are visible overloads, not magic, and nothing here treats them differently.
///
/// `ret` is `None` when the definition omitted its return annotation. A member is
/// registered as its own definition is reached and its signature never changes after
/// that, so an omitted return type stays `None` instead of standing in as a placeholder
/// type that would answer calls with the wrong answer. Every member must annotate its
/// return type (as it must annotate every parameter), and the omission is reported where
/// it bites: at a call to the member, or at its definition if nothing calls it.
#[derive(Debug, Clone)]
pub struct Overload {
    pub parameters: Vec<Type>,
    pub ret: Option<Type>,
}

/// Whether a declaration sits at the top level of a module or inside some body (a
/// function, method, or lambda). Only a top-level function is called by name through the
/// path that fills in a `Site` parameter, so this is what decides whether one is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nesting {
    TopLevel,
    Nested,
}

pub struct TypeChecker {
    env: Environment,
    // Registry of methods: (TypeName, MethodName) -> method definition
    methods: std::collections::HashMap<(String, String), MethodDef>,
    // Registry of sum types: TypeName -> Type::Sum
    sum_types: std::collections::HashMap<String, Type>,
    // Methods declared with `:=` ("setters"): they may mutate their receiver in place,
    // so calling one requires a `:=`-bound (mutable) receiver.
    setter_methods: std::collections::HashSet<(String, String)>,
    // The type oracle (see `TypeTable`): every inferred expression type, keyed by span,
    // populated as a side effect of `infer_expression` and returned by `check_program`.
    type_table: TypeTable,
    // Ad-hoc overload sets, keyed by name (function names AND operator symbols like
    // `"+"`/`"=="`). A name maps to all its candidate signatures; a call/operator use
    // resolves to the one whose parameter types EXACTLY match the argument types (no
    // implicit coercion). Built-in operator/`print` behavior lives here as `builtin`
    // members, so the standard operators are visible overloads, not compiler magic.
    overloads: std::collections::HashMap<String, Vec<Overload>>,
    // Top-level names that form a user overload set (operator-named, or 2+ defs).
    // A call to one of these resolves by exact argument type via `overloads` rather
    // than through a single `env` function binding. Computed in `check_program`.
    overloaded_names: std::collections::HashSet<String>,
    // The first overload member registered without a return type annotation, as
    // `(name, parameter types, definition span)`. A call to such a member is an error at
    // the call; this is what lets an uncalled one still be reported, at its definition.
    // Only the first is kept — one report per run is what the checker gives anyway.
    unannotated_overload_member: Option<(String, Vec<Type>, Span)>,
    // How many `describe` blocks enclose what is being checked. `expect` records into the
    // test reporter, so it is only legal where there is one — inside a `describe`.
    test_depth: usize,
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = TypeChecker {
            env: Environment::new(),
            methods: std::collections::HashMap::new(),
            sum_types: std::collections::HashMap::new(),
            setter_methods: std::collections::HashSet::new(),
            type_table: TypeTable::new(),
            overloads: std::collections::HashMap::new(),
            overloaded_names: std::collections::HashSet::new(),
            unannotated_overload_member: None,
            test_depth: 0,
        };

        // Add built-in sum types to the environment
        checker.add_builtins();
        // Register the built-in operator/`print` overloads (the standard operators
        // are visible overloads, not compiler magic).
        checker.add_builtin_overloads();

        checker
    }

    fn check_type_compatibility(
        &self,
        expected: &Type,
        got: &Type,
        span: &Span,
    ) -> Result<(), TypeError> {
        if Self::types_compatible(expected, got) {
            Ok(())
        } else {
            Err(TypeError::TypeMismatch {
                expected: Box::new(expected.clone()),
                got: Box::new(got.clone()),
                span: span.clone(),
            })
        }
    }

    /// Structural type compatibility with a `Generic` wildcard. A `Generic` on either
    /// side matches anything (no real type variables yet). For sum types this recurses
    /// into the variants so that the SAME sum type carrying a specialized payload in one
    /// value and a generic/`$` payload in another (e.g. `Ok("x")` vs `Ok($)`, both
    /// `Result`) stays compatible — the constructor result is specialized to the actual
    /// payload type (see `specialize_variant`) purely so a match can bind the payload at
    /// its real type; that specialization must NOT make two `Result` values incompatible.
    fn types_compatible(a: &Type, b: &Type) -> bool {
        match (a, b) {
            // A generic stands in for any type (forward-compat with future generics).
            (Type::Generic { .. }, _) | (_, Type::Generic { .. }) => true,
            (
                Type::Sum {
                    name: n1,
                    variants: v1,
                },
                Type::Sum {
                    name: n2,
                    variants: v2,
                },
            ) => {
                // Same sum type (by name) with structurally-compatible variants: same
                // variant names in order, payload fields pairwise compatible.
                n1 == n2
                    && v1.len() == v2.len()
                    && v1.iter().zip(v2).all(|(x, y)| {
                        x.name == y.name
                            && x.fields.len() == y.fields.len()
                            && x.fields
                                .iter()
                                .zip(&y.fields)
                                .all(|(fa, fb)| Self::types_compatible(fa, fb))
                    })
            }
            (Type::Array(e1), Type::Array(e2)) => Self::types_compatible(e1, e2),
            (Type::Map(k1, v1), Type::Map(k2, v2)) => {
                Self::types_compatible(k1, k2) && Self::types_compatible(v1, v2)
            }
            (Type::Set(e1), Type::Set(e2)) => Self::types_compatible(e1, e2),
            _ => a == b,
        }
    }
}
