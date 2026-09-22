//! Sum types and type resolution: the built-in `Result`, constructor applications, and
//! turning a written type into the checker's resolved one.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use crate::ast::Statement;
use crate::ast::walk::try_for_each_subexpression;
use std::ops::ControlFlow;

use super::*;

impl TypeChecker {
    pub(super) fn add_builtins(&mut self) {
        use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant, Type};

        // Unified Result{T} type with Ok and NotOk constructors
        // Ok(value) for success, NotOk(error) for failure
        let result_type = Type::Sum {
            name: RESULT_TYPE_NAME.to_string(),
            variants: vec![
                SumVariant {
                    name: OK.to_string(),
                    fields: vec![Type::Generic {
                        name: "T".to_string(),
                    }],
                },
                SumVariant {
                    name: NOT_OK.to_string(),
                    fields: vec![Type::Generic {
                        name: "E".to_string(),
                    }],
                },
            ],
        };

        // Register Result type in both env and sum_types registry. Its name — like every
        // built-in type's — is reserved (`ast::reserved_for`), so a program cannot rebind
        // it; the scalars need no env entry for that.
        self.sum_types
            .insert(RESULT_TYPE_NAME.to_string(), result_type.clone());
        self.env.define_builtin(RESULT_TYPE_NAME, result_type);

        // `Site` — the built-in call-site record (`file`/`line`/`column`/`excerpt`/`width`).
        // A named record type like any other, registered here rather than declared in a
        // corelib module so `:: Site` is nameable in any signature with no import; what
        // makes it special is only that a call FILLS IN a trailing `Site` argument with its
        // own location (see `ast::is_site_type`).
        self.env
            .define_builtin(crate::ast::SITE_TYPE_NAME, crate::ast::site_type());
    }

    /// Type-check a constructor application `variant(args...)` against the registered
    /// sum types. Returns `Ok(Some(sum_type))` if `variant` is a known constructor (after
    /// validating arity and payload types), `Ok(None)` if no sum type has that variant
    /// (so the caller can fall through to other interpretations), or `Err` on a mismatch.
    ///
    /// Only the matched variant's field types are cloned (a small Vec), not the whole
    /// registry — this runs on every call/constructor expression, so a full-map clone
    /// here would scale with program size x declared sum types.
    pub(super) fn check_constructor_call(
        &mut self,
        variant: &str,
        args: &[Expression],
        span: &Span,
    ) -> Result<Option<Type>, TypeError> {
        // Find the owning sum type and clone just what we need to drop the borrow.
        let found = self.sum_types.values().find_map(|sum_type| {
            if let Type::Sum { variants, .. } = sum_type
                && let Some(v) = variants.iter().find(|v| v.name == variant)
            {
                Some((sum_type.clone(), v.fields.clone()))
            } else {
                None
            }
        });
        let Some((sum_type, field_types)) = found else {
            return Ok(None);
        };

        if field_types.len() != args.len() {
            return Err(TypeError::WrongNumberOfArguments {
                expected: field_types.len(),
                got: args.len(),
                span: span.clone(),
            });
        }
        let mut arg_types = Vec::with_capacity(args.len());
        for (field_type, arg) in field_types.iter().zip(args.iter()) {
            let field_type = self.resolve_payload_type(field_type);
            let arg_type = self.infer_expression(arg)?;
            self.check_type_compatibility(&field_type, &arg_type, span)?;
            arg_types.push(arg_type);
        }

        // For a sum type with GENERIC payload positions (the built-in `Result`'s
        // `Ok(T)` / `NotOk(E)`), specialize the constructed variant's generic fields to
        // the concrete argument types. This lets `Ok("x")` carry `Text` (not the opaque
        // `T`), so a later `match` binds the payload at its real type and `.length` /
        // field access on it type-check — the front-end half of making `Ok(text)` /
        // `NotOk(text)` round-trip (codegen already preserves the payload's LLVM type).
        // Non-generic field types (user sum types, already concrete) pass through.
        let specialized = Self::specialize_variant(&sum_type, variant, &arg_types);
        Ok(Some(specialized))
    }

    /// Return `sum_type` with the `variant`'s generic payload fields replaced by the
    /// corresponding concrete `arg_types`. Only `Type::Generic` fields are substituted;
    /// already-concrete fields are left as declared, and other variants are untouched.
    /// Clones the sum type once and mutates only the matched variant's generic fields in
    /// place, rather than rebuilding every (mostly unchanged) sibling variant.
    pub(super) fn specialize_variant(sum_type: &Type, variant: &str, arg_types: &[Type]) -> Type {
        let mut specialized = sum_type.clone();
        if let Type::Sum { variants, .. } = &mut specialized
            && let Some(v) = variants.iter_mut().find(|v| v.name == variant)
        {
            for (i, field) in v.fields.iter_mut().enumerate() {
                if matches!(field, Type::Generic { .. })
                    && let Some(arg) = arg_types.get(i)
                {
                    *field = arg.clone();
                }
            }
        }
        specialized
    }

    /// Merge two already-compatible inferred types, preferring the more concrete payload
    /// at each sum-variant slot. For two `Type::Sum` of the same name and shape, each
    /// variant's payload fields become the concrete (non-`Generic`) side whenever either
    /// side is concrete — so a `?`/`if` whose branches are `Ok("x")` (its `NotOk` still
    /// generic) and `NotOk("e")` (its `Ok` still generic) yields
    /// `Result[Ok(Text), NotOk(Text)]`, letting BOTH arms bind their payload at the real
    /// type (the `getEnv`/`getOpt` shape). Any non-sum or differently-shaped pair returns
    /// `a` unchanged — the historical "take the first branch's type" behavior.
    pub(super) fn merge_types(a: Type, b: &Type) -> Type {
        use crate::ast::SumVariant;
        if let (
            Type::Sum {
                name: na,
                variants: va,
            },
            Type::Sum {
                name: nb,
                variants: vb,
            },
        ) = (&a, b)
            && na == nb
            && va.len() == vb.len()
            && va.iter().zip(vb).all(|(x, y)| x.name == y.name)
        {
            let variants = va
                .iter()
                .zip(vb)
                .map(|(x, y)| {
                    let fields = x
                        .fields
                        .iter()
                        .zip(&y.fields)
                        .map(|(fx, fy)| match fx {
                            Type::Generic { .. } => fy.clone(),
                            _ => fx.clone(),
                        })
                        .collect();
                    SumVariant {
                        name: x.name.clone(),
                        fields,
                    }
                })
                .collect();
            return Type::Sum {
                name: na.clone(),
                variants,
            };
        }
        a
    }

    /// If `variant` names a constructor of some registered sum type, return that
    /// sum type's name. Used to enforce globally-unique variant names.
    pub(super) fn sum_variant_owner(&self, variant: &str) -> Option<String> {
        for (type_name, sum_type) in &self.sum_types {
            if let Type::Sum { variants, .. } = sum_type
                && variants.iter().any(|v| v.name == variant)
            {
                return Some(type_name.clone());
            }
        }
        None
    }

    /// Substitutes a frozen self-reference placeholder (empty fields/variants, from before its own declaration completed) with the registered real type, recursing through array/map.
    pub(super) fn resolve_payload_type(&self, field_type: &Type) -> Type {
        match field_type {
            Type::Sum { name, variants } if variants.is_empty() => self
                .sum_types
                .get(name)
                .cloned()
                .unwrap_or_else(|| field_type.clone()),
            Type::Named { name, fields, .. } if fields.is_empty() => self
                .env
                .get_type(name)
                .filter(
                    |resolved| matches!(resolved, Type::Named { fields, .. } if !fields.is_empty()),
                )
                .unwrap_or_else(|| field_type.clone()),
            Type::Array(elem) => Type::Array(Box::new(self.resolve_payload_type(elem))),
            Type::Map(key, value) => Type::Map(
                Box::new(self.resolve_payload_type(key)),
                Box::new(self.resolve_payload_type(value)),
            ),
            _ => field_type.clone(),
        }
    }

    /// Resolve a parsed type annotation against registered types. The parser emits an
    /// unknown Capitalized name as `Type::Named { fields: [], .. }`; if it names a
    /// registered sum type, substitute the concrete definition so structural equality
    /// (`check_type_compatibility`) lines up with inferred constructor results.
    pub(super) fn resolve_type(&self, ty: &Type) -> Type {
        match ty {
            Type::Named { name, fields, .. } if fields.is_empty() => {
                // A registered sum type wins; otherwise a registered named RECORD type
                // (stored in `env` as its full `Named { fields, methods }`), so a
                // function/operator parameter typed `:: SomeRecord` carries its fields
                // and methods (field access / method dispatch in the body resolve).
                if let Some(sum) = self.sum_types.get(name) {
                    sum.clone()
                } else if let Some(named @ Type::Named { .. }) = self.env.get_type(name) {
                    named
                } else {
                    ty.clone()
                }
            }
            // A function type carries its parameter and return types unresolved from the
            // parser, so a named type nested inside one (`(Color) -> Bool`) must resolve
            // too — otherwise it would never line up with a concrete inferred type.
            Type::Function {
                parameters,
                return_type,
            } => Type::Function {
                parameters: parameters.iter().map(|p| self.resolve_type(p)).collect(),
                return_type: Box::new(self.resolve_type(return_type)),
            },
            // Recurse into the built-in composites so a nested named type in an annotation
            // (`[]Point`, `[|Point => Num|]`, `[|Point|]`) carries its resolved fields and
            // methods — otherwise a `Map(Point, …)` literal and its annotation disagree.
            Type::Array(elem) => Type::Array(Box::new(self.resolve_type(elem))),
            Type::Map(key, value) => Type::Map(
                Box::new(self.resolve_type(key)),
                Box::new(self.resolve_type(value)),
            ),
            Type::Set(elem) => Type::Set(Box::new(self.resolve_type(elem))),
            _ => ty.clone(),
        }
    }

    /// After the whole program is checked, reject a bare `:: Result` PARAMETER — of a
    /// top-level function, a `:=`/`=`-bound lambda, a method, or a function/lambda
    /// declared inside any of those bodies, at any nesting depth — that stays the raw,
    /// unspecialized built-in type through its own declaration (`resolve_type` gives
    /// every bare `:: Result` the same generic `Ok(T)`/`NotOk(E)` shape) but is matched
    /// directly in its OWN body, binding a payload (`Ok(x)`, not `Ok(_)`).
    ///
    /// Pinning such a parameter's payload from its callers — inferring it, rather than
    /// rejecting it — was this pass's first design and went through three review
    /// rounds, each of which found a NEW way a whole-program, name-keyed scan of call
    /// sites mis-attributes a match to the wrong declaration: a forwarding wrapper, a
    /// method or overload member (whose calls carry no receiver/member-independent
    /// argument to read), a nested function or `:=`-bound lambda the scan never visited,
    /// and a local rebinding that shadows the outer name. Closing each hole added a new
    /// one; the general fix — resolving a match's scrutinee through the checker's own
    /// scope-aware call resolution instead of a syntactic name scan — is sound in
    /// principle but is not a bug-fix-sized change. So this parameter's payload is
    /// instead treated as unrecoverable within this pass and rejected outright:
    /// `UnresolvedResultPayload`, naming the fix (match the producing call directly, or
    /// annotate). Every check here is purely LOCAL to one declaration's own body —
    /// nothing reads a caller, another declaration, or the whole program — so nothing
    /// can mis-attribute a match the way the caller-scanning design did.
    ///
    /// A parameter no site ever BINDS (`Ok(_)` / `NotOk(_)`) needs no payload type at
    /// all, so a function dispatched on the tag alone (`okTag`-style, every call passing
    /// a different concrete payload) is untouched. A `Result`'s payload type crossing a
    /// function boundary through its RETURN, including an OVERLOADED function's, is a
    /// different, already-sound mechanism — see [`Self::refine_overload_return_type`]
    /// and `check_function_declaration`'s own-`env`-binding refinement — since a
    /// function's return is pinned from its OWN body, never from a caller.
    pub(super) fn reject_unresolved_result_parameters(
        &mut self,
        program: &Program,
    ) -> Result<(), TypeError> {
        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) => {
                    if declaration.is_inert_corelib_placeholder() {
                        continue;
                    }
                    self.reject_if_bound_and_unresolved(
                        &declaration.name,
                        &declaration.parameters,
                        declaration.declared_parameters(),
                        &declaration.body,
                    )?;
                    self.reject_nested_result_parameters(&declaration.body)?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        // A method has no whole-signature `::` form of its own (no
                        // `binding_type`) — every parameter is annotated directly, or
                        // `check_type_methods` already rejected the declaration.
                        self.reject_if_bound_and_unresolved(
                            &method.name,
                            &method.parameters,
                            None,
                            &method.body,
                        )?;
                        self.reject_nested_result_parameters(&method.body)?;
                    }
                }
                Item::VariableDeclaration(declaration) => match &declaration.value {
                    // Checked directly, by its real binding name, rather than through
                    // `reject_nested_result_parameters`'s generic walk (which would also
                    // find this same top-level lambda, unhelpfully labeled "a lambda") —
                    // its own body is still handed to that walk, for anything nested
                    // further in. A lambda literal has no whole-signature form either
                    // (no `binding_type`): its parameters take their types only from
                    // their own annotations.
                    Expression::Lambda {
                        parameters, body, ..
                    } => {
                        self.reject_if_bound_and_unresolved(
                            &declaration.name,
                            parameters,
                            None,
                            body,
                        )?;
                        self.reject_nested_result_parameters(body)?;
                    }
                    other => self.reject_nested_result_parameters(other)?,
                },
            }
        }
        Ok(())
    }

    /// Every `:=`/`=`-bound or nested `FunctionDeclaration` lambda anywhere inside
    /// `body` — at any nesting depth, since a function or lambda declared inside one of
    /// those is checked exactly the same way, one level further in — each against its
    /// OWN parameters and OWN body only (see [`Self::reject_unresolved_result_parameters`]'s
    /// doc comment: no cross-declaration information is read here at all).
    fn reject_nested_result_parameters(&mut self, body: &Expression) -> Result<(), TypeError> {
        for candidate in nested_function_candidates(body) {
            self.reject_if_bound_and_unresolved(
                &candidate.name,
                candidate.parameters,
                candidate.declared,
                candidate.body,
            )?;
        }
        Ok(())
    }

    /// [`Self::reject_unresolved_result_parameters`]'s per-declaration work: every bare
    /// `:: Result` parameter of `parameters` that `body` matches directly and binds a
    /// payload from is `UnresolvedResultPayload`. `declared` is a whole-signature `::`
    /// annotation's parameter slots (`f :: (Result) -> Text = (result) => …`), read the
    /// same way [`crate::ast::FunctionDeclaration::parameter_type`] does — a parameter's
    /// own annotation wins, so this is consulted only when it has none of its own.
    fn reject_if_bound_and_unresolved(
        &self,
        name: &str,
        parameters: &[Parameter],
        declared: Option<&[Type]>,
        body: &Expression,
    ) -> Result<(), TypeError> {
        for (index, parameter) in parameters.iter().enumerate() {
            let annotation = parameter
                .type_annotation
                .as_ref()
                .or_else(|| declared.map(|slots| &slots[index]));
            let Some(annotation) = annotation else {
                continue;
            };
            if !is_unspecialized_result(&self.resolve_type(annotation)) {
                continue;
            }
            if let Some(span) = first_bound_span(&self.type_table, body, &parameter.name) {
                return Err(TypeError::UnresolvedResultPayload {
                    function: name.to_string(),
                    parameter: parameter.name.clone(),
                    span,
                });
            }
        }
        Ok(())
    }
}

/// Whether `ty` is the built-in `Result` exactly as declared — both payload positions
/// still the raw type variable, meaning no caller or constructor has specialized it yet
/// (a parameter's bare `:: Result` annotation always resolves to this).
fn is_unspecialized_result(ty: &Type) -> bool {
    matches!(ty, Type::Sum { name, variants }
        if name == crate::ast::RESULT_TYPE_NAME
            && variants
                .iter()
                .all(|v| v.fields.iter().all(|f| matches!(f, Type::Generic { .. }))))
}

/// A function or lambda declared somewhere inside another declaration's body —
/// [`TypeChecker::reject_nested_result_parameters`]'s own candidate to check next,
/// against its own parameters and its own body only. `declared` is its whole-signature
/// `::` annotation's parameter slots, when it has one (only a nested `FunctionDeclaration`
/// can; a lambda literal, named or not, never does).
struct NestedDeclaration<'a> {
    name: String,
    parameters: &'a [Parameter],
    declared: Option<&'a [Type]>,
    body: &'a Expression,
}

/// Every function/lambda-shaped construct anywhere inside `body`, at ANY nesting depth —
/// a `?`/`|` match inside one of THESE checks that construct's OWN parameters against
/// ITS OWN body when [`TypeChecker::reject_nested_result_parameters`] visits it, so a
/// nested declaration's `:: Result` parameter is covered by this pass exactly like a
/// top-level one, never by attributing anything found here to the ENCLOSING
/// declaration. A `:=`/`=`-bound lambda is named after its binding (found via its
/// enclosing `Block`, before the generic `Expression::Lambda` arm below reaches the same
/// lambda through the walk's own recursion into that binding's value — whichever
/// candidate is checked FIRST short-circuits `reject_nested_result_parameters` on a
/// rejection, so the well-named one always wins); an anonymous lambda literal (a bare
/// callback, say) has no name of its own to report, so `"a lambda"` names it instead.
/// `try_for_each_subexpression` already descends into a nested `FunctionDeclaration`'s
/// and a bound lambda's own body on its own, so one walk finds every depth — no
/// recursion needed here.
fn nested_function_candidates(body: &Expression) -> Vec<NestedDeclaration<'_>> {
    let mut candidates = Vec::new();
    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        match expression {
            Expression::Lambda {
                parameters, body, ..
            } => candidates.push(NestedDeclaration {
                name: "a lambda".to_string(),
                parameters,
                declared: None,
                body,
            }),
            Expression::Block { statements, .. } => {
                for statement in statements {
                    match statement {
                        Statement::Item(Item::FunctionDeclaration(nested)) => {
                            candidates.push(NestedDeclaration {
                                name: nested.name.clone(),
                                parameters: &nested.parameters,
                                declared: nested.declared_parameters(),
                                body: &nested.body,
                            });
                        }
                        Statement::Item(Item::VariableDeclaration(local))
                            if let Expression::Lambda {
                                parameters, body, ..
                            } = &local.value =>
                        {
                            candidates.push(NestedDeclaration {
                                name: local.name.clone(),
                                parameters,
                                declared: None,
                                body,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
    candidates
}

/// Every name in `body` that is a DIRECT copy of `parameter_name` — a local `=`/`:=`
/// binding whose own value is a bare read of an already-known alias (starting from
/// `parameter_name` itself), chased transitively (`renamed = result` then
/// `renamedAgain = renamed` both count). This is the one piece of the checker's own
/// scope-aware resolution [`first_bound_span`] borrows, and only for this one narrow
/// shape (a same-named identifier copied straight across an `=`/`:=`, not through a
/// call, a ternary, or any other transformation) — those remain out of reach for this
/// pass, same as before. Capped at a handful of iterations: a real alias chain is a
/// couple of names deep at most, and a program that somehow keeps introducing new
/// aliases from this set forever is not a shape this pass needs to chase further.
fn direct_aliases_of(body: &Expression, parameter_name: &str) -> std::collections::HashSet<String> {
    let mut aliases = std::collections::HashSet::new();
    aliases.insert(parameter_name.to_string());
    for _ in 0..8 {
        let mut grew = false;
        let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
            if let Expression::Block { statements, .. } = expression {
                for statement in statements {
                    if let Statement::Item(Item::VariableDeclaration(local)) = statement
                        && let Expression::Identifier { name, .. } = &local.value
                        && aliases.contains(name)
                        && aliases.insert(local.name.clone())
                    {
                        grew = true;
                    }
                }
            }
            ControlFlow::Continue(())
        });
        if !grew {
            break;
        }
    }
    aliases
}

/// The span of the first payload binding (`Ok(x)`/`NotOk(x)`, not `Ok(_)`) on a direct
/// match of `parameter_name`, OR ONE OF ITS DIRECT ALIASES (see [`direct_aliases_of`]),
/// anywhere in `body` — [`TypeChecker::reject_if_bound_and_unresolved`]'s evidence that
/// this parameter's payload is read as something concrete, with nothing in this pass
/// able to say what.
///
/// This is a PURELY LOCAL check — nothing here reads a caller, another declaration, or
/// the type any OTHER expression carries — so unlike a whole-program call scan, nothing
/// here can mis-attribute a match to the wrong declaration. The one thing it still must
/// get right within this one body is a LOCAL shadow: a nested function's or lambda's own
/// same-named parameter, or a local `=`/`:=`/nested-function declaration reusing one of
/// these names, both make that name refer to a DIFFERENT value from that point on (each
/// shadowing declaration's own region is excluded by byte range — cheaper than threading
/// scope state through every expression variant, since nothing outside a scope can read
/// a name it shadows). A shadow that reassigns the name to an ALREADY-CONCRETE `Result`
/// (`result := Ok(aNum)`) type-checks fine and would otherwise look, by name, identical
/// to a genuine read of the still-generic parameter — `type_table`, the checker's own
/// first-pass record of each identifier occurrence's REAL type, disambiguates: a site
/// counts only when its scrutinee's own recorded type is exactly as unspecialized as the
/// target parameter's.
fn first_bound_span(
    type_table: &TypeTable,
    body: &Expression,
    parameter_name: &str,
) -> Option<Span> {
    use crate::ast::{NOT_OK, OK};

    let aliases = direct_aliases_of(body, parameter_name);
    let is_alias = |name: &str| aliases.contains(name);

    let mut found: Vec<Span> = Vec::new();
    let mut shadows: Vec<Span> = Vec::new();
    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        match expression {
            Expression::Match {
                expression: scrutinee,
                arms,
                ..
            } if matches!(
                scrutinee.as_ref(),
                Expression::Identifier { name, .. } if is_alias(name)
            ) =>
            {
                let Expression::Identifier { span, .. } = scrutinee.as_ref() else {
                    unreachable!("matched above")
                };
                if !type_table.get(span).is_some_and(is_unspecialized_result) {
                    return ControlFlow::Continue(());
                }
                for arm in arms {
                    if let Pattern::Constructor {
                        name: constructor,
                        arguments,
                        ..
                    } = &arm.pattern
                        && let [Pattern::Identifier { span: binding, .. }] = arguments.as_slice()
                        && matches!(constructor.as_str(), OK | NOT_OK)
                    {
                        found.push(binding.clone());
                    }
                }
            }
            Expression::Lambda {
                parameters, span, ..
            } if parameters.iter().any(|p| is_alias(&p.name)) => {
                shadows.push(span.clone());
            }
            Expression::Block { statements, span } => {
                for statement in statements {
                    match statement {
                        // A nested function's own PARAMETER of this name shadows it for
                        // that function's whole span (its body, but also the function
                        // literal itself — nothing outside can reach in either way).
                        Statement::Item(Item::FunctionDeclaration(nested))
                            if nested.parameters.iter().any(|p| is_alias(&p.name)) =>
                        {
                            shadows.push(nested.span.clone());
                        }
                        // A nested function's or a local `=`/`:=` binding's own NAME
                        // rebinds it from that declaration onward, through the rest of
                        // THIS block (statements execute in the order written, and nest
                        // no further scope of their own).
                        Statement::Item(Item::FunctionDeclaration(nested))
                            if is_alias(&nested.name) =>
                        {
                            shadows.push(Span {
                                start: nested.span.start,
                                end: span.end,
                                file: span.file,
                            });
                        }
                        // Excludes the very declarations `direct_aliases_of` followed to
                        // build `aliases` in the first place (`renamed = result`): that
                        // introduces a new alias, it does not shadow one — only a local
                        // whose OWN name is an alias AND whose value ISN'T itself a bare
                        // read of one (a genuine rebind, `result := Ok(5)`) shadows it.
                        Statement::Item(Item::VariableDeclaration(local))
                            if is_alias(&local.name)
                                && !matches!(
                                    &local.value,
                                    Expression::Identifier { name, .. } if is_alias(name)
                                ) =>
                        {
                            shadows.push(Span {
                                start: local.span.start,
                                end: span.end,
                                file: span.file,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });

    // Every found binding still loses to a shadow that turns out to cover it — the walk
    // above visits a `Match` before it necessarily knows every shadow in the SAME body
    // (a shadowing declaration can sit textually after the match that reads the
    // parameter, e.g. inside a later statement of the same block), so the exclusion is
    // applied once, after the whole walk, over every candidate — not just the first,
    // which could be the one a later shadow excludes while an earlier, unshadowed
    // candidate still stands.
    found.into_iter().find(|span| {
        !shadows.iter().any(|shadow| {
            shadow.file == span.file && shadow.start <= span.start && span.end <= shadow.end
        })
    })
}

/// The `Result` type with a CONCRETE `Ok` payload of `elem` (and a `$`/Unit `NotOk`
/// for the "absent" case). `find`/`at` return this so a downstream match binds the
/// element at its real type and exhaustiveness/codegen size it correctly.
pub(super) fn result_of(elem: Type) -> Type {
    use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant};
    Type::Sum {
        name: RESULT_TYPE_NAME.to_string(),
        variants: vec![
            SumVariant {
                name: OK.to_string(),
                fields: vec![elem],
            },
            SumVariant {
                name: NOT_OK.to_string(),
                fields: vec![Type::Unit],
            },
        ],
    }
}
