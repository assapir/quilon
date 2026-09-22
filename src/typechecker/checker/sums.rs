//! Sum types and type resolution: the built-in `Result`, constructor applications, and
//! turning a written type into the checker's resolved one.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

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

    /// After the whole program is checked, pin the payload type of a `Result`-typed
    /// PARAMETER that stays the raw, unspecialized built-in type through its own
    /// declaration (`resolve_type` gives every bare `:: Result` the same generic
    /// `Ok(T)`/`NotOk(E)` shape) but is matched directly in its function's body. The
    /// source is the same one [`Self::check_constructor_call`] draws from for a
    /// constructor's own result — every caller's argument at that position, already
    /// type-checked and sitting in the oracle by now, since a plain function's callers
    /// (there is no hoisting) are always checked at or after its own declaration.
    ///
    /// Two callers disagreeing over a position's payload is a `TypeMismatch` at the
    /// second one. A parameter whose function is called nowhere, with a body that still
    /// binds a payload identifier, is `UnresolvedResultPayload` — reported rather than
    /// left for codegen to default. A parameter that IS called, but never with a
    /// concrete payload at some position (only the OTHER variant is ever passed), keeps
    /// that position `Generic` — exactly the existing, sound "unconstructed variant"
    /// case a local binding's own construction sites already leave generic.
    ///
    /// Overloaded functions are skipped: their return-type equivalent of this gap is
    /// closed instead by [`Self::refine_overload_return_type`], which runs per member as
    /// its own body is checked — a parameter has no analogous per-member registration to
    /// refine, so it is pinned here, once, after every call site is known.
    ///
    /// This teaches the ORACLE (what codegen reads) the bound payload's real type; it does
    /// not re-run the checker's own first pass over the body, which already resolved
    /// every call/operator inside it against the STILL-generic parameter. So the payload
    /// binding itself must stay in a position `Generic` already tolerates at that first
    /// pass — a sum constructor argument (`Done(text)`, checked for compatibility, which a
    /// `Generic` always satisfies) — not one requiring an EXACT overload match against it
    /// (`text + "!"`, a `Text` method call): those fail at the first pass before this ever
    /// runs, with the ordinary `NoMatchingOverload`/`UnknownMember` a `Generic` operand
    /// gets anywhere else. A caller that needs the payload for more than that passes it to
    /// a match on the producing call directly, or extracts it again from an already-pinned
    /// return value (see `examples/result_helper.qn`).
    pub(super) fn pin_result_parameters(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            let Item::FunctionDeclaration(declaration) = item else {
                continue;
            };
            if declaration.is_inert_corelib_placeholder()
                || self.overloaded_names.contains(&declaration.name)
            {
                continue;
            }
            for (index, parameter) in declaration.parameters.iter().enumerate() {
                let Some(annotation) = &parameter.type_annotation else {
                    continue;
                };
                if !is_unspecialized_result(&self.resolve_type(annotation)) {
                    continue;
                }
                let sites = result_parameter_matches(&declaration.body, &parameter.name);
                if sites.is_empty() {
                    continue;
                }
                self.pin_one_result_parameter(program, declaration, index, &sites)?;
            }
        }
        Ok(())
    }

    /// [`Self::pin_result_parameters`]'s per-parameter work: fold every caller's argument
    /// at `index` into a pinned `Ok`/`NotOk` payload, then teach every matched site in
    /// `sites` that pinned type.
    fn pin_one_result_parameter(
        &mut self,
        program: &Program,
        declaration: &FunctionDeclaration,
        index: usize,
        sites: &[ResultParameterMatch],
    ) -> Result<(), TypeError> {
        use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant};

        // Only a variant some site actually BINDS needs a single, agreed-on payload type —
        // a discarded payload (`Ok(_)`) never reads its value, so callers may legitimately
        // pass it different concrete types at every call (`okTag`-style dispatch on the tag
        // alone). Unifying a position nothing binds would reject that as a false conflict.
        let needs_ok = sites.iter().any(|site| site.ok_binding.is_some());
        let needs_not_ok = sites.iter().any(|site| site.not_ok_binding.is_some());

        let mut ok_pin: Option<Type> = None;
        let mut not_ok_pin: Option<Type> = None;
        let mut called = false;
        for call in calls_to(program, &declaration.name) {
            let Expression::Call { arguments, .. } = call else {
                unreachable!("calls_to only ever returns Call expressions")
            };
            if index >= arguments.len() {
                continue;
            }
            called = true;
            let argument = &arguments[index];
            let Some(Type::Sum { name, variants }) = self.type_table.get(argument.span()).cloned()
            else {
                continue;
            };
            if name != RESULT_TYPE_NAME {
                continue;
            }
            if needs_ok && let Some(variant) = variants.iter().find(|v| v.name == OK) {
                unify_result_pin(&mut ok_pin, &variant.fields[0], argument.span())?;
            }
            if needs_not_ok && let Some(variant) = variants.iter().find(|v| v.name == NOT_OK) {
                unify_result_pin(&mut not_ok_pin, &variant.fields[0], argument.span())?;
            }
        }

        if !called {
            let unbound = sites
                .iter()
                .find_map(|site| site.ok_binding.as_ref().or(site.not_ok_binding.as_ref()));
            return match unbound {
                Some(span) => Err(TypeError::UnresolvedResultPayload {
                    function: declaration.name.clone(),
                    parameter: declaration.parameters[index].name.clone(),
                    span: span.clone(),
                }),
                None => Ok(()),
            };
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

/// One `?`/`|` match, within a function's body, whose SCRUTINEE is a bare read of one of
/// the function's own `Result`-typed parameters — what [`pin_result_parameters`] looks
/// for. `ok_binding`/`not_ok_binding` are the payload sub-pattern's own span, when that
/// arm binds a name (`Ok(text)`) rather than discarding the payload (`Ok(_)`); matching a
/// DERIVED value (a call, a field, a renamed local) is out of reach for this pass —
/// matching the producing call directly, or annotating, still works.
struct ResultParameterMatch {
    scrutinee_span: Span,
    ok_binding: Option<Span>,
    not_ok_binding: Option<Span>,
}

/// Every `Match` expression in `body` whose scrutinee is a bare read of `parameter_name`.
fn result_parameter_matches(body: &Expression, parameter_name: &str) -> Vec<ResultParameterMatch> {
    use crate::ast::{NOT_OK, OK};

    let mut sites = Vec::new();
    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        if let Expression::Match {
            expression: scrutinee,
            arms,
            ..
        } = expression
            && let Expression::Identifier { name, span } = scrutinee.as_ref()
            && name == parameter_name
        {
            let mut site = ResultParameterMatch {
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
                    && let [Pattern::Identifier { span: binding, .. }] = arguments.as_slice()
                {
                    match constructor.as_str() {
                        OK => site.ok_binding = Some(binding.clone()),
                        NOT_OK => site.not_ok_binding = Some(binding.clone()),
                        _ => {}
                    }
                }
            }
            sites.push(site);
        }
        ControlFlow::Continue(())
    });
    sites
}

/// Every plain (non-member) call to `name` anywhere in `program` — a top-level function
/// body, a global's initializer, or a type's method body — as the call expression itself,
/// so its arguments' spans are ready-made oracle keys.
fn calls_to<'a>(program: &'a Program, name: &str) -> Vec<&'a Expression> {
    let mut calls = Vec::new();
    let mut visit = |expression: &'a Expression| {
        let _: ControlFlow<()> = try_for_each_subexpression(expression, &mut |e| {
            if let Expression::Call {
                function,
                member_call: false,
                ..
            } = e
                && let Expression::Identifier { name: callee, .. } = function.as_ref()
                && callee == name
            {
                calls.push(e);
            }
            ControlFlow::Continue(())
        });
    };
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => visit(&declaration.body),
            Item::VariableDeclaration(declaration) => visit(&declaration.value),
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    visit(&method.body);
                }
            }
        }
    }
    calls
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
