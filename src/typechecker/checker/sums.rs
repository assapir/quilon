//! Sum types and type resolution: the built-in `Result`, constructor applications, and
//! turning a written type into the checker's resolved one.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use crate::ast::Statement;
use crate::ast::walk::try_for_each_subexpression;
use std::collections::HashSet;
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

    /// After the whole program is checked, pin the payload type of a bare `:: Result`
    /// PARAMETER — of a top-level function, a `:=`/`=`-bound lambda, or a function/lambda
    /// declared inside either of those, at any nesting depth — that stays the raw,
    /// unspecialized built-in type through its own declaration (`resolve_type` gives
    /// every bare `:: Result` the same generic `Ok(T)`/`NotOk(E)` shape) but is matched
    /// directly in its OWN body, binding a payload (`Ok(x)`, not `Ok(_)`).
    ///
    /// The payload is pinned from every DIRECT call site's already-checked argument type
    /// at that position — the same source [`Self::check_constructor_call`] draws from
    /// for a constructor's own result. Two callers disagreeing over a position's payload
    /// is a `TypeMismatch` at the second one. A bound position no direct call informs is
    /// left `Generic` (the historical, sound-for-`Num` default an unconstructed local
    /// variant's own slot already gets) rather than rejected, UNLESS the declaration's
    /// name is referenced NOWHERE in the whole program at all — a truly dead
    /// declaration, where `UnresolvedResultPayload` is reported instead of silently
    /// deferring to that default. A method's or an overload member's own bound
    /// parameter carries no such call site to pin from at all (a member call's argument
    /// can't be attributed to one receiver-independent signature, and a bare call to an
    /// overloaded name doesn't say which member it fills), so it is
    /// `UnresolvedResultPayload` unconditionally.
    ///
    /// This design is narrower than it once was: pinning from a whole-program,
    /// name-keyed scan of call sites went through several review rounds, each finding a
    /// new way it mis-attributes a match to the wrong declaration (a forwarding
    /// wrapper's own uninformative call, a nested function or lambda the scan never
    /// visited, a local rebinding that shadows the outer name) — all fixed below, by
    /// scoping every match/shadow check to purely LOCAL, per-declaration information
    /// (nothing here reads what ANOTHER declaration's body contains) and reserving the
    /// whole-program scan strictly for reading a DIRECT call's own, already-checked
    /// argument type. A position no site ever BINDS (`Ok(_)` / `NotOk(_)`) needs no
    /// payload type at all, so a function dispatched on the tag alone (`okTag`-style,
    /// every call passing a different concrete payload) is untouched either way.
    ///
    /// A `Result`'s payload type crossing a function boundary through its RETURN,
    /// including an OVERLOADED function's, is a different, already-sound mechanism —
    /// see [`Self::refine_overload_return_type`] and `check_function_declaration`'s
    /// own-`env`-binding refinement — since a function's return is pinned from its OWN
    /// body, never from a caller.
    ///
    /// A bound position no direct call informs is rejected instead of left `Generic`
    /// when the binding itself is passed straight into a sum-variant constructor OR a
    /// plain function whose own slot there is concrete and isn't `Num` (see
    /// [`Self::pin_one_result_parameter`] and `flows_into_a_non_num_constructor`) — the
    /// one shape that reaches codegen as `Num`'s `f64` stored against a
    /// differently-shaped slot, an internal error (a constructor's `coerce_payload`, or
    /// a plain call's own LLVM module verification) rather than a silent mismatch.
    ///
    /// One gap this leaves: a parameter FORWARDED into another function's own bare
    /// `:: Result` parameter, rather than called with a concrete `Ok`/`NotOk` argument
    /// directly, is not informed by that forwarding call (its argument is an
    /// `Identifier`, not a constructor application, so it carries no payload type of its
    /// own to read) — the forwarded-to parameter is then left `Generic` exactly like a
    /// never-called one, and `oracle::value_repr_type` renders it as `Num`'s `f64`
    /// regardless of its real payload. This is caught only when the forwarded-to
    /// parameter's OWN body feeds it into a concrete-typed constructor or a plain
    /// function call ([`Self::slot_is_unsafe_for_generic`]'s guard, deliberately NOT
    /// resolving an OVERLOADED name's own member — see its doc comment); a forwarded-to
    /// parameter merely returned, used in arithmetic, or itself fed into an overloaded
    /// call is not. The residual risk differs by shape: returned or used in arithmetic,
    /// it silently computes as if it really were `Num`; fed into an overloaded call
    /// (or any other statically-fixed non-`Num` slot this guard doesn't reach), the
    /// mismatch is still an internal error at codegen (LLVM's own module verification,
    /// or `coerce_payload`), not a silent wrong answer — the same class of failure this
    /// whole fix exists to catch earlier, just not caught here.
    /// Closing it fully would need either tracing a payload's type through every call in
    /// between (undoing the very whole-program mis-attribution risk the narrowing above
    /// exists to avoid) or rejecting every such forwarding parameter outright (which
    /// would also reject a parameter dispatched on the tag alone through a chain of
    /// forwarders); left as a known, non-regressing limitation for now.
    pub(super) fn pin_result_parameters(&mut self, program: &Program) -> Result<(), TypeError> {
        let index = index_program_calls(program);

        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) => {
                    if declaration.is_inert_corelib_placeholder() {
                        continue;
                    }
                    let pinnable = !self.overloaded_names.contains(&declaration.name);
                    self.pin_result_parameters_of(
                        &declaration.name,
                        &declaration.parameters,
                        declaration.declared_parameters(),
                        &declaration.body,
                        pinnable,
                        &index,
                    )?;
                    self.pin_nested_result_parameters(&declaration.body, &index)?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        // A method has no whole-signature `::` form of its own (no
                        // `binding_type`) — every parameter is annotated directly, or
                        // `check_type_methods` already rejected the declaration. Never
                        // pinnable: a member call's argument can't be attributed to one
                        // receiver-independent signature.
                        self.pin_result_parameters_of(
                            &method.name,
                            &method.parameters,
                            None,
                            &method.body,
                            false,
                            &index,
                        )?;
                        self.pin_nested_result_parameters(&method.body, &index)?;
                    }
                }
                Item::VariableDeclaration(declaration) => match &declaration.value {
                    // Checked directly, by its real binding name, rather than through
                    // `pin_nested_result_parameters`'s generic walk (which would also
                    // find this same top-level lambda, unhelpfully labeled "a lambda") —
                    // its own body is still handed to that walk, for anything nested
                    // further in. A lambda literal has no whole-signature form either
                    // (no `binding_type`): its parameters take their types only from
                    // their own annotations. Pinnable — called by its binding's name
                    // exactly like a `FunctionDeclaration`, and never overloaded.
                    Expression::Lambda {
                        parameters, body, ..
                    } => {
                        self.pin_result_parameters_of(
                            &declaration.name,
                            parameters,
                            None,
                            body,
                            true,
                            &index,
                        )?;
                        self.pin_nested_result_parameters(body, &index)?;
                    }
                    other => self.pin_nested_result_parameters(other, &index)?,
                },
            }
        }
        Ok(())
    }

    /// Every `:=`/`=`-bound or nested `FunctionDeclaration` lambda, and every locally
    /// declared type's method, anywhere inside `body` — at any nesting depth, since one
    /// declared inside one of those is checked exactly the same way, one level further
    /// in — each against its own parameters and own body only (see
    /// [`Self::pin_result_parameters`]'s doc comment: nothing here reads what ANOTHER
    /// declaration's body contains). A nested function or lambda is pinnable (its name
    /// is never in `overloaded_names`, which only a top-level `FunctionDeclaration`
    /// joins); a nested type's method never is, same as a top-level type's.
    fn pin_nested_result_parameters(
        &mut self,
        body: &Expression,
        index: &CallIndex<'_>,
    ) -> Result<(), TypeError> {
        for candidate in nested_function_candidates(body) {
            self.pin_result_parameters_of(
                &candidate.name,
                candidate.parameters,
                candidate.declared,
                candidate.body,
                !candidate.is_method,
                index,
            )?;
        }
        Ok(())
    }

    /// [`Self::pin_result_parameters`]'s per-declaration work, shared by a top-level
    /// function, a method, a lambda, and every nested candidate: every bare `:: Result`
    /// parameter of `parameters` that `body` matches directly. `pinnable` is false for a
    /// method or an overload member, where a bound payload is reported unconditionally
    /// rather than pinning attempted (see the doc comment above). `declared` is a
    /// whole-signature `::` annotation's parameter slots (`f :: (Result) -> Text =
    /// (result) => …`), read the same way
    /// [`crate::ast::FunctionDeclaration::parameter_type`] does — a parameter's own
    /// annotation wins, so this is consulted only when it has none of its own.
    fn pin_result_parameters_of(
        &mut self,
        name: &str,
        parameters: &[Parameter],
        declared: Option<&[Type]>,
        body: &Expression,
        pinnable: bool,
        call_index: &CallIndex<'_>,
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
            let sites = result_parameter_matches(&self.type_table, body, &parameter.name);
            if sites.is_empty() {
                continue;
            }
            if !pinnable {
                if let Some(span) = first_binding(&sites) {
                    return Err(TypeError::UnresolvedResultPayload {
                        function: name.to_string(),
                        parameter: parameter.name.clone(),
                        span: span.clone(),
                    });
                }
                continue;
            }
            self.pin_one_result_parameter(name, index, &parameter.name, &sites, body, call_index)?;
        }
        Ok(())
    }

    /// [`Self::pin_result_parameters_of`]'s pinnable-parameter case: fold every direct
    /// caller's argument at `index` into a pinned `Ok`/`NotOk` payload, then teach every
    /// matched site in `sites` that pinned type.
    fn pin_one_result_parameter(
        &mut self,
        name: &str,
        index: usize,
        parameter_name: &str,
        sites: &[ResultParameterMatch],
        body: &Expression,
        call_index: &CallIndex<'_>,
    ) -> Result<(), TypeError> {
        let CallIndex {
            calls_by_name,
            referenced_names,
        } = call_index;
        use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant};

        // Only a variant some site actually BINDS needs a single, agreed-on payload type
        // — a discarded payload (`Ok(_)`) never reads its value, so callers may
        // legitimately pass it different concrete types at every call (`okTag`-style
        // dispatch on the tag alone). Unifying a position nothing binds would reject
        // that as a false conflict.
        let needs_ok = sites.iter().any(|site| site.ok_binding.is_some());
        let needs_not_ok = sites.iter().any(|site| site.not_ok_binding.is_some());

        let no_calls = Vec::new();
        let calls = calls_by_name.get(name).unwrap_or(&no_calls);
        let mut ok_pin: Option<Type> = None;
        let mut not_ok_pin: Option<Type> = None;
        for call in calls {
            let Expression::Call { arguments, .. } = call else {
                unreachable!("calls_by_name only ever indexes Call expressions")
            };
            if index >= arguments.len() {
                continue;
            }
            let argument = &arguments[index];
            let Some(Type::Sum {
                name: sum_name,
                variants,
            }) = self.type_table.get(argument.span()).cloned()
            else {
                continue;
            };
            if sum_name != RESULT_TYPE_NAME {
                continue;
            }
            if needs_ok && let Some(variant) = variants.iter().find(|v| v.name == OK) {
                unify_result_pin(&mut ok_pin, &variant.fields[0], argument.span())?;
            }
            if needs_not_ok && let Some(variant) = variants.iter().find(|v| v.name == NOT_OK) {
                unify_result_pin(&mut not_ok_pin, &variant.fields[0], argument.span())?;
            }
        }

        // A variant no direct caller informs is normally left `Generic`, falling back to
        // codegen's numeric default — sound as long as nothing ever needs that binding's
        // REAL representation. It doesn't when the binding is itself passed straight into
        // a sum-variant constructor whose field at that position is concrete and isn't
        // `Num`: codegen then has a `Num`-shaped (`f64`) value to store into a slot sized
        // for something else, an internal error `coerce_payload` catches but which a
        // clean compile should never have reached. Caught here instead, by name, at the
        // binding that would cause it.
        if needs_ok
            && ok_pin.is_none()
            && let Some((binding_name, binding_span)) =
                sites.iter().find_map(|s| s.ok_binding.as_ref())
            && flows_into_a_non_num_constructor(self, body, binding_name)
        {
            return Err(TypeError::UnresolvedResultPayload {
                function: name.to_string(),
                parameter: parameter_name.to_string(),
                span: binding_span.clone(),
            });
        }
        if needs_not_ok
            && not_ok_pin.is_none()
            && let Some((binding_name, binding_span)) =
                sites.iter().find_map(|s| s.not_ok_binding.as_ref())
            && flows_into_a_non_num_constructor(self, body, binding_name)
        {
            return Err(TypeError::UnresolvedResultPayload {
                function: name.to_string(),
                parameter: parameter_name.to_string(),
                span: binding_span.clone(),
            });
        }

        // Reject only when the name is referenced NOWHERE in the whole program at all —
        // a truly dead declaration, whose payload no caller (direct or otherwise) can
        // ever teach this pass. A declaration that IS referenced but never with a
        // concrete argument at THIS position keeps that position generic instead: the
        // same historical, sound-for-`Num` default an unconstructed local variant's own
        // slot already gets (a position genuinely read with something else needs a
        // caller demonstrating that, which pins it here instead of leaving it generic).
        if !referenced_names.contains(name) {
            if let Some(span) = first_binding(sites) {
                return Err(TypeError::UnresolvedResultPayload {
                    function: name.to_string(),
                    parameter: parameter_name.to_string(),
                    span: span.clone(),
                });
            }
            return Ok(());
        }

        if ok_pin.is_none() && not_ok_pin.is_none() {
            return Ok(());
        }

        let pinned = Type::Sum {
            name: RESULT_TYPE_NAME.to_string(),
            variants: vec![
                SumVariant {
                    name: OK.to_string(),
                    fields: vec![ok_pin.unwrap_or_else(|| Type::Generic {
                        name: "T".to_string(),
                    })],
                },
                SumVariant {
                    name: NOT_OK.to_string(),
                    fields: vec![not_ok_pin.unwrap_or_else(|| Type::Generic {
                        name: "E".to_string(),
                    })],
                },
            ],
        };
        for site in sites {
            self.type_table
                .insert(site.scrutinee_span.clone(), pinned.clone());
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

/// A function, lambda, or method declared somewhere inside another declaration's body —
/// [`TypeChecker::pin_nested_result_parameters`]'s own candidate to check next, against
/// its own parameters and its own body only. `declared` is its whole-signature `::`
/// annotation's parameter slots, when it has one (only a nested `FunctionDeclaration`
/// can; a lambda literal or a method, named or not, never does). `is_method` is true
/// only for a locally-declared type's method — never pinnable, unlike every other
/// candidate here (see [`TypeChecker::pin_result_parameters`]'s doc comment).
struct NestedDeclaration<'a> {
    name: String,
    parameters: &'a [Parameter],
    declared: Option<&'a [Type]>,
    body: &'a Expression,
    is_method: bool,
}

/// Every function/lambda/method-shaped construct anywhere inside `body`, at ANY nesting
/// depth — a `?`/`|` match inside one of THESE checks that construct's OWN parameters
/// against ITS OWN body when [`TypeChecker::pin_nested_result_parameters`] visits it, so
/// a nested declaration's `:: Result` parameter is covered by this pass exactly like a
/// top-level one, never by attributing anything found here to the ENCLOSING
/// declaration. A `:=`/`=`-bound lambda is named after its binding (found via its
/// enclosing `Block`, before the generic `Expression::Lambda` arm below reaches the same
/// lambda through the walk's own recursion into that binding's value — whichever
/// candidate is checked FIRST short-circuits `pin_nested_result_parameters` on a
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
                is_method: false,
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
                                is_method: false,
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
                                is_method: false,
                            });
                        }
                        // A type declared locally (inside a function's own body) has
                        // methods exactly like a top-level one's — each is its own
                        // candidate, never a method has a whole-signature form, and
                        // never pinnable (a member call's argument can't be attributed
                        // to one receiver-independent signature).
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                candidates.push(NestedDeclaration {
                                    name: method.name.clone(),
                                    parameters: &method.parameters,
                                    declared: None,
                                    body: &method.body,
                                    is_method: true,
                                });
                            }
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

/// One `?`/`|` match, within a declaration's body, whose SCRUTINEE is a bare read of one
/// of its own `Result`-typed parameters (or a direct alias of one — see
/// [`result_parameter_matches`]) — what [`TypeChecker::pin_result_parameters`] looks
/// for. `ok_binding`/`not_ok_binding` are the payload sub-pattern's own name and span,
/// when that arm binds a name (`Ok(text)`) rather than discarding the payload (`Ok(_)`);
/// matching a DERIVED value (a call, a field, a renamed local's further transformation)
/// is out of reach for this pass — matching the producing call directly, or annotating,
/// still works.
struct ResultParameterMatch {
    scrutinee_span: Span,
    ok_binding: Option<(String, Span)>,
    not_ok_binding: Option<(String, Span)>,
}

/// The span of the first payload binding (`Ok(x)`/`NotOk(x)`, not `Ok(_)`) among `sites`,
/// if any binds one at all — the span an unpinnable declaration's `UnresolvedResultPayload`
/// names, and the same evidence a pinnable one falls back to naming if it turns out to be
/// truly unreachable (see [`TypeChecker::pin_one_result_parameter`]).
fn first_binding(sites: &[ResultParameterMatch]) -> Option<&Span> {
    sites.iter().find_map(|site| {
        site.ok_binding
            .as_ref()
            .or(site.not_ok_binding.as_ref())
            .map(|(_, span)| span)
    })
}

/// One name's status, valid only within `scope` (a byte-range, file-qualified region of
/// source): either an ALIAS of `parameter_name` (`alive: true`) or a local shadow of that
/// SAME NAME by something unrelated (`alive: false`) — a nested function's own parameter,
/// a nested declaration's own name, or a genuine rebind (`result := Ok(5)`). Keyed by
/// NAME (not just span) so two DIFFERENT names that happen to share a byte range never
/// answer for each other — see [`resolve_alive`].
struct AliasEntry {
    name: String,
    scope: Span,
    alive: bool,
}

/// Whether `name` is currently an alias of the original parameter at source location
/// `at`: among every entry FOR THAT EXACT NAME whose `scope` contains `at`, the
/// NARROWEST one wins (an inner, more specific scope always shadows an outer one) — and
/// its `alive` flag is the answer. No entry for the name at that location at all means
/// "never introduced here", which is not an alias.
///
/// This is intentionally ORDER-INDEPENDENT: every entry's `scope` already encodes exactly
/// where it applies (e.g., a rebind's own scope starts at ITS OWN span, not the whole
/// enclosing block, so it can never retroactively shadow an earlier read), so resolving
/// against the complete, unordered set of entries collected over the whole walk gives the
/// same answer regardless of which order [`result_parameter_matches`] visited them in —
/// unlike a flat, un-keyed shadow list, which silently answers for ANY name whose site
/// happens to fall within a shadow's byte range, an unrelated name's shadow can never
/// answer for a different name here.
fn resolve_alive(entries: &[AliasEntry], name: &str, at: &Span) -> bool {
    entries
        .iter()
        .filter(|entry| {
            entry.name == name
                && entry.scope.file == at.file
                && entry.scope.start <= at.start
                && at.end <= entry.scope.end
        })
        .min_by_key(|entry| entry.scope.end - entry.scope.start)
        .is_some_and(|entry| entry.alive)
}

/// A `?`/`|` match found during the walk, before it's known whether its scrutinee's name
/// still resolves to `parameter_name` at that point — resolved by [`resolve_alive`] only
/// once the whole walk (and so every [`AliasEntry`] it could depend on) is complete.
struct MatchCandidate {
    scrutinee_name: String,
    scrutinee_span: Span,
    ok_binding: Option<(String, Span)>,
    not_ok_binding: Option<(String, Span)>,
}

/// Every `?`/`|` match in `body` whose scrutinee is a bare read of `parameter_name`, or a
/// name that is a DIRECT copy of it (`renamed = result`, chased transitively) — this
/// pass's evidence that a payload is read as something concrete, with nothing in the
/// parameter's own declaration able to say what.
///
/// This is a PURELY LOCAL walk — nothing here reads a caller, another declaration, or
/// the type any OTHER expression carries — so unlike a whole-program call scan, nothing
/// here can mis-attribute a match to the wrong declaration. The one thing it still must
/// get right within this one body is a LOCAL shadow: a nested function's or lambda's own
/// same-named parameter, or a local `=`/`:=`/nested-function declaration reusing one of
/// these names, both make that name refer to a DIFFERENT value from that point on. Each
/// is recorded as an [`AliasEntry`] scoped to exactly where it applies, keyed by its OWN
/// name — so a coincidental name collision between two UNRELATED nested declarations
/// (each with their own local of the same name) can never make one's shadow answer for
/// the other's, the way a flat, name-blind byte-range list once could. A shadow that
/// reassigns the name to an ALREADY-CONCRETE `Result` (`result := Ok(aNum)`) type-checks
/// fine and would otherwise look, by name, identical to a genuine read of the still-generic
/// parameter — `type_table`, the checker's own first-pass record of each identifier
/// occurrence's REAL type, disambiguates: a site counts only when its scrutinee's own
/// recorded type is exactly as unspecialized as the target parameter's.
fn result_parameter_matches(
    type_table: &TypeTable,
    body: &Expression,
    parameter_name: &str,
) -> Vec<ResultParameterMatch> {
    use crate::ast::{NOT_OK, OK};

    // The parameter itself is always its own alias, everywhere in its own body.
    let mut entries = vec![AliasEntry {
        name: parameter_name.to_string(),
        scope: body.span().clone(),
        alive: true,
    }];
    let mut candidates: Vec<MatchCandidate> = Vec::new();

    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        match expression {
            Expression::Match {
                expression: scrutinee,
                arms,
                ..
            } => {
                if let Expression::Identifier { name, span } = scrutinee.as_ref() {
                    let mut candidate = MatchCandidate {
                        scrutinee_name: name.clone(),
                        scrutinee_span: span.clone(),
                        ok_binding: None,
                        not_ok_binding: None,
                    };
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
                            let binding = (binding_name.clone(), binding_span.clone());
                            match constructor.as_str() {
                                OK => candidate.ok_binding = Some(binding),
                                NOT_OK => candidate.not_ok_binding = Some(binding),
                                _ => {}
                            }
                        }
                    }
                    candidates.push(candidate);
                }
            }
            // An anonymous lambda's own parameter shadows any outer name it reuses, for
            // its whole span (parameters and body alike).
            Expression::Lambda {
                parameters, span, ..
            } => {
                for parameter in parameters {
                    entries.push(AliasEntry {
                        name: parameter.name.clone(),
                        scope: span.clone(),
                        alive: false,
                    });
                }
            }
            Expression::Block { statements, span } => {
                for statement in statements {
                    match statement {
                        // A nested function's own PARAMETER of this name shadows it for
                        // that function's whole span (its body, but also the function
                        // literal itself — nothing outside can reach in either way).
                        // Pushed unconditionally: a name that was never an alias to begin
                        // with just gets a harmless tombstone nothing ever resolves as
                        // alive anyway.
                        Statement::Item(Item::FunctionDeclaration(nested)) => {
                            for parameter in &nested.parameters {
                                entries.push(AliasEntry {
                                    name: parameter.name.clone(),
                                    scope: nested.span.clone(),
                                    alive: false,
                                });
                            }
                            // The declaration's own NAME rebinds it from here onward,
                            // through the rest of THIS block (statements execute in the
                            // order written, and nest no further scope of their own).
                            entries.push(AliasEntry {
                                name: nested.name.clone(),
                                scope: Span {
                                    start: nested.span.start,
                                    end: span.end,
                                    file: span.file,
                                },
                                alive: false,
                            });
                        }
                        // A locally-declared type's method, same as a nested function's
                        // own parameter above — each method is checked as its own
                        // candidate (see `nested_function_candidates`), so a match
                        // inside one that reuses this name must not also be attributed
                        // to the enclosing declaration's parameter.
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                for parameter in &method.parameters {
                                    entries.push(AliasEntry {
                                        name: parameter.name.clone(),
                                        scope: method.span.clone(),
                                        alive: false,
                                    });
                                }
                            }
                        }
                        // A bare `newName = anAlias` copy introduces a second name for
                        // the SAME value, valid from here to the end of THIS block;
                        // anything else reusing an existing alias's name (`result :=
                        // Ok(5)`) is a genuine rebind, and shadows it the same way.
                        // Resolving `name`'s aliveness HERE (rather than deferring it
                        // too) is safe: entries an earlier sibling statement in this
                        // same block pushed are already present (this loop runs them in
                        // written order), and nothing a LATER sibling or a different
                        // scope could push would ever apply at this exact span anyway.
                        Statement::Item(Item::VariableDeclaration(local)) => {
                            let rest_of_block = Span {
                                start: local.span.start,
                                end: span.end,
                                file: span.file,
                            };
                            let alive = matches!(
                                &local.value,
                                Expression::Identifier { name, .. }
                                    if resolve_alive(&entries, name, &local.span)
                            );
                            entries.push(AliasEntry {
                                name: local.name.clone(),
                                scope: rest_of_block,
                                alive,
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
        .into_iter()
        .filter(|candidate| {
            resolve_alive(
                &entries,
                &candidate.scrutinee_name,
                &candidate.scrutinee_span,
            )
        })
        .filter(|candidate| {
            type_table
                .get(&candidate.scrutinee_span)
                .is_some_and(is_unspecialized_result)
        })
        .map(|candidate| ResultParameterMatch {
            scrutinee_span: candidate.scrutinee_span,
            ok_binding: candidate.ok_binding,
            not_ok_binding: candidate.not_ok_binding,
        })
        .collect()
}

/// A one-time index over the whole program, shared by every candidate parameter
/// [`TypeChecker::pin_result_parameters`] considers (rather than re-walking the program
/// once per candidate): `calls_by_name[callee]` is every plain (non-member) call
/// expression naming `callee`, and `referenced_names` is every name any `Identifier`
/// expression anywhere reads (a superset of the callees above, since a call's own
/// function position is itself an `Identifier`) — the signal
/// [`TypeChecker::pin_one_result_parameter`] uses to tell a truly dead declaration
/// (referenced nowhere at all, where a bound payload is `UnresolvedResultPayload`) from
/// one that IS referenced but never with a concrete argument at some position (left
/// generic there instead). Bundled into one struct so passing it down through
/// `pin_result_parameters_of` and `pin_nested_result_parameters` costs one argument, not
/// two.
struct CallIndex<'a> {
    calls_by_name: HashMap<&'a str, Vec<&'a Expression>>,
    referenced_names: HashSet<&'a str>,
}

fn index_program_calls(program: &Program) -> CallIndex<'_> {
    let mut calls_by_name: HashMap<&str, Vec<&Expression>> = HashMap::new();
    let mut referenced_names: HashSet<&str> = HashSet::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => {
                index_expression(&declaration.body, &mut calls_by_name, &mut referenced_names)
            }
            Item::VariableDeclaration(declaration) => index_expression(
                &declaration.value,
                &mut calls_by_name,
                &mut referenced_names,
            ),
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    index_expression(&method.body, &mut calls_by_name, &mut referenced_names);
                }
            }
        }
    }
    CallIndex {
        calls_by_name,
        referenced_names,
    }
}

/// [`index_program_calls`]'s per-expression work — its own named, explicitly-lifetimed
/// function rather than a closure, which a plain `impl FnMut` cannot express here: the
/// borrowed `Expression`s these two maps collect must outlive the call that finds them.
fn index_expression<'a>(
    expression: &'a Expression,
    calls_by_name: &mut HashMap<&'a str, Vec<&'a Expression>>,
    referenced_names: &mut HashSet<&'a str>,
) {
    let _: ControlFlow<()> = try_for_each_subexpression(expression, &mut |e| {
        match e {
            Expression::Identifier { name, .. } => {
                referenced_names.insert(name.as_str());
            }
            Expression::Call {
                function,
                member_call: false,
                ..
            } => {
                if let Expression::Identifier { name, .. } = function.as_ref() {
                    calls_by_name.entry(name.as_str()).or_default().push(e);
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
}

/// Fold a caller-supplied field type into `pinned`: the first concrete type wins, a later
/// caller must agree with it exactly (a `TypeMismatch` at ITS argument — the
/// disagreement's own site), and a still-generic field (a caller forwarding an equally
/// unpinned value) teaches nothing.
fn unify_result_pin(pinned: &mut Option<Type>, field: &Type, span: &Span) -> Result<(), TypeError> {
    if matches!(field, Type::Generic { .. }) {
        return Ok(());
    }
    match pinned {
        Some(existing) if existing != field => Err(TypeError::TypeMismatch {
            expected: Box::new(existing.clone()),
            got: Box::new(field.clone()),
            span: span.clone(),
        }),
        Some(_) => Ok(()),
        None => {
            *pinned = Some(field.clone());
            Ok(())
        }
    }
}

/// Whether `binding_name` is passed, anywhere in `body`, as a bare argument to a
/// sum-variant constructor call whose field at that position is CONCRETE and isn't
/// `Num` — the one shape [`TypeChecker::pin_one_result_parameter`] cannot safely leave
/// `Generic`: `Num` is safe because Generic's own codegen default (`f64`) already IS
/// `Num`'s representation, so nothing downstream can tell the difference; anything else
/// (`Text`, a record, another sum, an array) has its own, DIFFERENT representation, and
/// a `Generic` value flowing into that slot is exactly what makes `coerce_payload` fail.
/// A purely local scan, like [`result_parameter_matches`] itself: it reads only what
/// `binding_name` is passed to, not what any OTHER declaration's body contains — a
/// function's or a lambda's own concrete-typed parameter is the same, already-accepted
/// forwarding gap [`TypeChecker::pin_result_parameters`]'s doc comment names, since a
/// user function's declared parameter type isn't looked up here.
impl TypeChecker {
    /// Whether `binding_name` is passed, anywhere in `body`, as a bare argument to EITHER
    /// a sum-variant constructor OR a plain (non-overloaded) function whose slot at that
    /// position is CONCRETE and isn't `Num` — see [`flows_into_a_non_num_constructor`]'s
    /// doc comment for why that shape is unsafe to leave `Generic`. An overloaded name's
    /// own member isn't resolved here (`self.env.get_type` answers only a plain
    /// function's single registered type, and correctly answers nothing useful for one),
    /// so a payload forwarded into an overload set keeps the same accepted, non-widened
    /// forwarding gap it already had.
    fn slot_is_unsafe_for_generic(&self, callee: &str, position: usize) -> bool {
        // A `Result` slot — specialized or not — shares `Result`'s own ONE canonical
        // `{ ptr, i64 }` representation regardless of payload (`pack_result_payload`),
        // so passing an equally-unpinned payload into it forces no REAL representation
        // decision the way a genuinely concrete (`Text`, a record, …) field does; this
        // is the accepted Result-to-Result forwarding gap this pass already documents,
        // not a new concrete-slot danger.
        let concrete_non_num = |field: &Type| {
            !matches!(field, Type::Generic { .. } | Type::Num)
                && !matches!(field, Type::Sum { name, .. } if name == crate::ast::RESULT_TYPE_NAME)
        };
        self.sum_types.values().any(|sum_type| {
            let Type::Sum { variants, .. } = sum_type else {
                return false;
            };
            variants
                .iter()
                .find(|v| v.name == callee)
                .and_then(|v| v.fields.get(position))
                .is_some_and(concrete_non_num)
        }) || matches!(
            self.env.get_type(callee),
            Some(Type::Function { parameters, .. })
                if parameters.get(position).is_some_and(concrete_non_num)
        )
    }
}

/// Whether `binding_name` is passed, anywhere in `body`, as a bare argument to a call
/// whose slot at that position is CONCRETE and isn't `Num` (see
/// [`TypeChecker::slot_is_unsafe_for_generic`]) — the one shape
/// [`TypeChecker::pin_one_result_parameter`] cannot safely leave `Generic`: `Num` is safe
/// because Generic's own codegen default (`f64`) already IS `Num`'s representation, so
/// nothing downstream can tell the difference; anything else (`Text`, a record, another
/// sum, an array) has its own, DIFFERENT representation, and a `Generic` value flowing
/// into that slot is exactly what makes `coerce_payload` (a constructor) or LLVM's own
/// module verifier (a plain function call) fail. A purely local scan, like
/// [`result_parameter_matches`] itself: it reads only what `binding_name` is passed to,
/// not what any OTHER declaration's body contains — an OVERLOADED function's own member
/// isn't resolved (see [`TypeChecker::slot_is_unsafe_for_generic`]), which remains the
/// same, already-accepted forwarding gap [`TypeChecker::pin_result_parameters`]'s doc
/// comment names.
fn flows_into_a_non_num_constructor(
    checker: &TypeChecker,
    body: &Expression,
    binding_name: &str,
) -> bool {
    let mut found = false;
    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        if found {
            return ControlFlow::Break(());
        }
        if let Expression::Call {
            function,
            arguments,
            member_call: false,
            ..
        } = expression
            && let Expression::Identifier { name: callee, .. } = function.as_ref()
        {
            for (position, argument) in arguments.iter().enumerate() {
                if let Expression::Identifier { name, .. } = argument
                    && name == binding_name
                    && checker.slot_is_unsafe_for_generic(callee, position)
                {
                    found = true;
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    });
    found
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
