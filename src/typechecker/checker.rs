// Type checker implementation

use crate::ast::{BinaryOperator, UnaryOperator};
use crate::ast::{
    Expression, FunctionDeclaration, InterpolationPart, Item, MatchArm, Parameter, Pattern,
    Program, Type, VariableDeclaration,
};
use crate::ast::{literal_number, type_label};
use crate::lexer::Span;

// The checker's methods live in child modules — one per checking area — as further
// `impl TypeChecker` blocks. Children of this file rather than siblings under
// `typechecker`, so the state declared below stays private to the checker: a child can
// reach its ancestor's private items, a sibling could not.
mod aliasing;
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

use aliasing::{ResultAliasing, ValueAliasing};
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
    /// A `:=` binding whose value aliases an `=` binding or a parameter — the value would
    /// become writable while an `=` binding still reaches it. `aliased` names the binding
    /// (or parameter) whose guarantee the alias would break.
    MutableAliasOfImmutable {
        name: String,
        aliased: String,
        parameter: bool,
        span: Span,
    },
    /// An `=` binding whose value aliases a `:=` binding — writes through the mutable
    /// binding would change the `=`-bound value underneath.
    ImmutableAliasOfMutable {
        name: String,
        aliased: String,
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
    /// `expect` outside a `describe` block, where there is no run to record into.
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
    /// operator makes. `lambda_parameter_shadows_receiver` marks a body that contains a
    /// lambda parameter named `it` — the write may target the lambda's own value, and
    /// the diagnostic names the shadowing.
    MutatingMethodDeclaredImmutable {
        type_name: String,
        method: String,
        lambda_parameter_shadows_receiver: bool,
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
        /// Where each argument is, so the report can label each with its type.
        arg_spans: Vec<Span>,
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
    /// A function parameter with no type annotation. A parameter type must be written
    /// down; it is not assumed to be `Num`. (A lambda passed to a built-in collection
    /// method is the exception — its parameter type comes from the element type.)
    UnannotatedParameter {
        function: String,
        parameter: String,
        span: Span,
    },
    /// A definition with more or fewer parameters than the function type it must match —
    /// `f :: (Num, Num) -> Num = (a :: Num) => a`, or a lambda handed to a position that
    /// states a different arity. `subject` names the offending definition.
    SignatureArity {
        subject: String,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// A lambda parameter left unannotated where the position it sits in states no function
    /// type to take it from. Carries the overload set that the other arguments left open,
    /// when that is what stopped the target type from being known.
    UninferableLambdaParameter {
        parameter: String,
        open_overload: Option<String>,
        span: Span,
    },
    /// A self-recursive call to a function with no `-> T` return annotation: the call
    /// needs to already know what the function returns, and an unannotated function's
    /// return type is only known once its body — which the recursive call sits inside —
    /// is fully checked. Anchored at the function's own definition, not the call, since
    /// the fix is the same wherever the recursive call sits.
    RecursiveFunctionNeedsReturnType {
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
    /// A `?`/`|` match that does not cover its scrutinee: a sum type with `missing`
    /// variants left unlisted, or any other type with no catch-all arm at all (nothing
    /// enumerates the values of a `Num`, so only `_` — or a binding — covers the rest).
    /// Total either way, so no value can fall off the end of a match.
    NonExhaustiveMatch {
        scrutinee: Box<Type>,
        missing: Vec<String>,
        span: Span,
    },
    /// A constructor pattern naming something the scrutinee's sum type has no variant for
    /// (`Ok(x)` against a `Color`). Codegen dispatches on a tag looked up by name, so this
    /// is caught here rather than left to fail at run time.
    UnknownConstructor {
        constructor: String,
        sum: String,
        known: Vec<String>,
        span: Span,
    },
    /// A constructor pattern against a scrutinee that is not a sum type at all
    /// (`5 ? | Ok(x) => …`). Only a sum has variants to dispatch on.
    ConstructorPatternOnNonSum {
        constructor: String,
        got: Box<Type>,
        span: Span,
    },
    /// The `^` entry point declared an unsupported parameter signature. The only
    /// accepted forms are `()`, `(args :: []Text)`, and
    /// `(args :: []Text, env :: [|Text => Text|])`. Rejected here (not in codegen) so
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
    /// `recv.name(...)` where `name` is not a member of the receiver's type. The name is
    /// looked for on that type alone, so a function of the same name is never a fallback
    /// — it would otherwise hijack the call. `in_scope` says whether such a function is
    /// there to point the reader at; `receiver` is what the caller wrote as the receiver,
    /// where that is a plain name, so the advice can spell out the call to write instead.
    UnknownMember {
        type_name: String,
        member: String,
        in_scope: bool,
        receiver: Option<String>,
        more_arguments: bool,
        span: Span,
    },
    /// `name(recv, ...)` where the receiver's type has a member `name` and the top level
    /// has no such function. The plain form names the top-level namespace alone, so the
    /// advice spells out the `.` call that reaches the member; `receiver` is what the
    /// caller wrote, where that is a plain name.
    MethodCalledAsFunction {
        type_name: String,
        member: String,
        receiver: Option<String>,
        more_arguments: bool,
        span: Span,
    },
    /// An output built-in (`print`/`eprint`/`write`) was handed a value with no rendering —
    /// a function.
    NotRenderable {
        name: String,
        got: Box<Type>,
        span: Span,
    },
}

/// What the position a lambda sits in states about its type — the target of **contextual
/// typing**. Only a `Declared` function type of matching arity can type the parameters the
/// lambda leaves unannotated; the other variants are what an "annotate it" error reports as
/// missing, so the reason travels as the situation rather than as prose.
#[derive(Clone, Copy)]
pub(crate) enum LambdaTarget<'a> {
    /// The type this position states. A function type of matching arity types the lambda;
    /// anything else leaves its unannotated parameters with nothing to take.
    Declared(&'a Type),
    /// The position states nothing — a lambda in a plain expression, an array element, a
    /// sum payload.
    None,
    /// A call to an overload set the other arguments did not narrow to one member, so which
    /// signature this position has is not decided yet.
    OpenOverload(&'a str),
}

impl<'a> LambdaTarget<'a> {
    /// The type the position states, if it states one.
    fn stated(self) -> Option<&'a Type> {
        match self {
            Self::Declared(ty) => Some(ty),
            _ => None,
        }
    }

    /// The error an unannotated `parameter` gets here — nothing stated a type to give it.
    fn uninferable(self, parameter: &Parameter) -> TypeError {
        TypeError::UninferableLambdaParameter {
            parameter: parameter.name.clone(),
            open_overload: match self {
                Self::OpenOverload(name) => Some(name.to_string()),
                _ => None,
            },
            span: parameter.span.clone(),
        }
    }
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
        // A function-typed parameter matches an argument of the same shape — same arity,
        // pairwise-matching parameters, matching result. So a set may overload on the
        // closure it takes, and the member a lambda argument resolves to is the one whose
        // signature it was typed against.
        (
            Type::Function {
                parameters: pa,
                return_type: ra,
            },
            Type::Function {
                parameters: pb,
                return_type: rb,
            },
        ) => {
            pa.len() == pb.len()
                && pa.iter().zip(pb).all(|(a, b)| types_match(a, b))
                && types_match(ra, rb)
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
// `pub` so it doesn't leak through the public `Environment::lookup` signature.
pub struct Symbol {
    type_: Type,
    mutable: bool,
    /// The declaration (function/method/lambda) whose body this binding belongs to — the
    /// id `TypeChecker::current_declaration` held when it was defined. Bindings owned by
    /// a declaration die when it returns, which is what lets its result classify as fresh.
    owner: u64,
    /// The bindings this binding's VALUE may alias beyond the binding itself: for a
    /// parameter, its own argument slot; for an `=` binding, whatever its initializer
    /// aliased. Empty for a fresh value.
    value_aliasing: ValueAliasing,
    /// For a binding holding a classified named function: which bindings/arguments a
    /// call's result may alias. `None` means unknown — a call is then assumed to alias
    /// every argument.
    result_aliasing: Option<ResultAliasing>,
    /// The receiver `it` of a `:=`-declared (setter) method, which is mutable at every
    /// call site.
    setter_receiver: bool,
    /// A payload-less constant (a nullary sum variant value): shared, but with no
    /// writable interior, so every use counts as fresh.
    constant: bool,
}

#[derive(Debug, Clone)]
pub struct Environment {
    scopes: Vec<HashMap<String, Symbol>>,
}

/// A method's signature and body: (parameters, return type, body expression). The return
/// type is always resolved — `check_type_methods` infers it from the body when the method
/// has no `-> Type` annotation, so a registered method always has a concrete one.
type MethodDef = (Vec<Parameter>, Type, Expression);

/// The **type oracle**: a side-table mapping each expression's — and each function
/// parameter's — source `Span` to the `Type` the checker inferred for it. Produced by
/// `check_program` and consumed by codegen so that READ sites (array indexing,
/// record-field access, match-arm results) recover the *declared* element / field / result
/// type instead of guessing `f64` from a runtime LLVM value, and so a parameter typed from
/// context rather than from a written annotation still has a type to lower. Spans are
/// unique per node (every AST node carries its own source span), so they make a stable,
/// AST-agnostic key. See the consumer-side wrapper `codegen::TypeOracle`.
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
    /// Which bindings/arguments this member's result may alias (see [`ResultAliasing`]).
    /// Built-in members return fresh values (`Some(default)`); a user member starts as
    /// `None` (assumed to alias every argument) and is classified when its body is
    /// checked.
    pub(crate) result_aliasing: Option<ResultAliasing>,
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
    // How many `describe` blocks, and how many `it` cases, enclose what is being checked.
    // `expect` marks the running CASE failed and the case's close is what tallies that, so it
    // is only legal inside an `it` — which in turn is only compiled inside a `describe`.
    test_depth: usize,
    case_depth: usize,
    // Result aliasing per declared method, keyed like `methods`. Slot 0 is the receiver,
    // slot i+1 the i-th explicit parameter — matching a member call's argument list. A
    // method absent here (a scalar-returning one) returns a fresh value.
    method_result_aliasing: std::collections::HashMap<(String, String), ResultAliasing>,
    // Aliasing of each `?`/`|` match expression, keyed by its span — the union of its
    // arms', computed in `check_match` WHILE each arm's pattern bindings are still in
    // scope (a later walk could no longer resolve them).
    match_aliasing: std::collections::HashMap<Span, ValueAliasing>,
    // Aliasing bookkeeping: each function/method/lambda body gets a fresh declaration id
    // (`declaration_counter` is the source; ids grow inward, so a nested declaration's id
    // is always greater than its encloser's). `current_declaration` is the body being
    // checked — 0 at the top level.
    declaration_counter: u64,
    current_declaration: u64,
    // `(name, definition span)` of the non-overloaded, unannotated function currently
    // having its body checked, when it has no `-> T` return type — its own name is left
    // UNDEFINED in `env` for that window (see `check_function_declaration`), so a
    // self-recursive call to it resolves here instead of as `UndefinedVariable`, and is
    // reported as `RecursiveFunctionNeedsReturnType` rather than assuming `Num`. `None`
    // outside that window, or while checking an annotated/overloaded function; saved and
    // restored around a nested declaration's own check so it still names the right
    // function afterward.
    pending_return_type: Option<(String, Span)>,
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
            method_result_aliasing: std::collections::HashMap::new(),
            match_aliasing: std::collections::HashMap::new(),
            declaration_counter: 0,
            current_declaration: 0,
            test_depth: 0,
            case_depth: 0,
            pending_return_type: None,
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
