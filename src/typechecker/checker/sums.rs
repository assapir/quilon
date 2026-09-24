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

    /// Pin a bare `:: Result` parameter's payload from its own call sites, once the
    /// whole program's calls and overloads are known: a variant some site binds and
    /// reads, but no site ever passes, is `UnresolvedResultPayload`; binding it unread
    /// needs no payload type. A method call is attributed by receiver type and an
    /// overloaded call by the member it resolved to (`CallOrigin`), not by name. A
    /// top-level function unreachable from `^` is skipped entirely, matching codegen's
    /// own tree-shaking.
    pub(super) fn pin_result_parameters(&mut self, program: &Program) -> Result<(), TypeError> {
        let calls_by_name = index_program_calls(program);
        let reachable = crate::ast::reachability::reachable_functions(program);

        for candidate in declaration_candidates(program) {
            if candidate.top_level_function
                && !reachable
                    .as_ref()
                    .is_none_or(|reachable| reachable.contains(candidate.name.as_str()))
            {
                continue;
            }
            let origin = match &candidate.owning_type {
                Some(owning_type) => {
                    self.method_call_origin(owning_type, &candidate.name, candidate.parameters)
                }
                None if candidate.top_level_function
                    && self.overloaded_names.contains(&candidate.name) =>
                {
                    CallOrigin::Overload {
                        name: candidate.name.clone(),
                        parameter_types: self
                            .resolved_parameter_types(candidate.parameters, candidate.declared),
                    }
                }
                None => CallOrigin::Named(candidate.name.clone()),
            };
            self.pin_result_parameters_of(
                &candidate.name,
                candidate.parameters,
                candidate.declared,
                candidate.body,
                &origin,
                &calls_by_name,
            )?;
        }
        Ok(())
    }

    /// The resolved types of `parameters` (or `declared`'s whole-signature slots where a
    /// parameter has no annotation of its own) — an overload member's own key into
    /// `overloads`/`overload_call_args`.
    fn resolved_parameter_types(
        &self,
        parameters: &[Parameter],
        declared: Option<&[Type]>,
    ) -> Vec<Type> {
        parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let annotation = parameter
                    .type_annotation
                    .as_ref()
                    .or_else(|| declared.map(|slots| &slots[index]))
                    .expect("an overload member's every parameter is annotated");
                self.resolve_type(annotation)
            })
            .collect()
    }

    /// A method's calls come from its qualified `"Type.method"` overload set if it has
    /// one, otherwise from `owning_type` and its own name.
    fn method_call_origin(
        &self,
        owning_type: &str,
        method_name: &str,
        method_parameters: &[Parameter],
    ) -> CallOrigin {
        let qualified_name = format!("{owning_type}.{method_name}");
        if self.overloads.contains_key(&qualified_name) {
            CallOrigin::Overload {
                name: qualified_name,
                parameter_types: self.resolved_parameter_types(method_parameters, None),
            }
        } else {
            CallOrigin::Method {
                type_name: owning_type.to_string(),
                method_name: method_name.to_string(),
            }
        }
    }

    /// Pin every bare `:: Result` parameter of `parameters` that `body` matches
    /// directly. `declared` is a whole-signature `::` annotation's parameter slots,
    /// consulted only when a parameter has no annotation of its own.
    fn pin_result_parameters_of(
        &mut self,
        name: &str,
        parameters: &[Parameter],
        declared: Option<&[Type]>,
        body: &Expression,
        origin: &CallOrigin,
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
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
            self.pin_one_result_parameter(
                name,
                index,
                &parameter.name,
                &sites,
                origin,
                calls_by_name,
            )?;
        }
        Ok(())
    }

    /// The argument spans of every call [`CallOrigin`] attributes to a declaration.
    fn call_argument_spans(
        &self,
        origin: &CallOrigin,
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
    ) -> Vec<Vec<Span>> {
        match origin {
            CallOrigin::Named(name) => calls_by_name
                .get(name.as_str())
                .into_iter()
                .flatten()
                .filter_map(|call| match call {
                    Expression::Call { arguments, .. } => {
                        Some(arguments.iter().map(|a| a.span().clone()).collect())
                    }
                    _ => None,
                })
                .collect(),
            CallOrigin::Method {
                type_name,
                method_name,
            } => self
                .method_call_args
                .get(&(type_name.clone(), method_name.clone()))
                .cloned()
                .unwrap_or_default(),
            CallOrigin::Overload {
                name,
                parameter_types,
            } => self
                .overload_call_args
                .get(name)
                .into_iter()
                .flatten()
                .filter(|(member_parameters, _)| member_parameters == parameter_types)
                .map(|(_, spans)| spans.clone())
                .collect(),
        }
    }

    /// Fold every call's argument at `index` into a pinned `Ok`/`NotOk` payload, and
    /// teach it to every matched site in `sites`.
    fn pin_one_result_parameter(
        &mut self,
        name: &str,
        index: usize,
        parameter_name: &str,
        sites: &[MatchCandidate<'_>],
        origin: &CallOrigin,
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
    ) -> Result<(), TypeError> {
        use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant};

        // Only a variant some site actually BINDS needs a single, agreed-on payload type
        // — a discarded payload (`Ok(_)`) never reads its value, so callers may
        // legitimately pass it different concrete types at every call (`okTag`-style
        // dispatch on the tag alone). Unifying a position nothing binds would reject
        // that as a false conflict.
        let needs_ok = sites.iter().any(|site| site.ok_binding.is_some());
        let needs_not_ok = sites.iter().any(|site| site.not_ok_binding.is_some());

        let mut ok_pin: Option<Type> = None;
        let mut not_ok_pin: Option<Type> = None;
        for call_args in self.call_argument_spans(origin, calls_by_name) {
            let Some(argument_span) = call_args.get(index) else {
                continue;
            };
            let Some(Type::Sum {
                name: sum_name,
                variants,
            }) = self.type_table.get(argument_span).cloned()
            else {
                continue;
            };
            if sum_name != RESULT_TYPE_NAME {
                continue;
            }
            if needs_ok && let Some(variant) = variants.iter().find(|v| v.name == OK) {
                unify_result_pin(&mut ok_pin, &variant.fields[0], argument_span)?;
            }
            if needs_not_ok && let Some(variant) = variants.iter().find(|v| v.name == NOT_OK) {
                unify_result_pin(&mut not_ok_pin, &variant.fields[0], argument_span)?;
            }
        }

        // A variant no caller demonstrates has no payload type at all: reject the FIRST
        // site that actually READS it (naming that exact binding), and otherwise leave
        // it alone — a bound-but-unread payload needs no type, since nothing ever
        // observes its representation.
        if needs_ok
            && ok_pin.is_none()
            && let Some((_, span, _)) = sites
                .iter()
                .filter_map(|s| s.ok_binding.as_ref())
                .find(|(binding_name, _, arm_body)| is_payload_read(arm_body, binding_name))
        {
            return Err(TypeError::UnresolvedResultPayload {
                function: name.to_string(),
                parameter: parameter_name.to_string(),
                variant: OK.to_string(),
                span: span.clone(),
            });
        }
        if needs_not_ok
            && not_ok_pin.is_none()
            && let Some((_, span, _)) = sites
                .iter()
                .filter_map(|s| s.not_ok_binding.as_ref())
                .find(|(binding_name, _, arm_body)| is_payload_read(arm_body, binding_name))
        {
            return Err(TypeError::UnresolvedResultPayload {
                function: name.to_string(),
                parameter: parameter_name.to_string(),
                variant: NOT_OK.to_string(),
                span: span.clone(),
            });
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

/// A declaration's calls: a plain function or a bound lambda by NAME (looked up in the
/// whole-program `calls_by_name` index); a method by its RECEIVER's type (recorded by
/// `check_call`); an overload member by its OWN parameter types (recorded by
/// `resolve_overload`, since several members share the overloaded name).
enum CallOrigin {
    Named(String),
    Method {
        type_name: String,
        method_name: String,
    },
    Overload {
        name: String,
        parameter_types: Vec<Type>,
    },
}

/// A function, lambda, or method to check on its own — its own parameters and body
/// only. `declared` is a whole-signature `::` annotation's parameter slots, when it has
/// one. `owning_type` is the enclosing type's name for a method, `None` otherwise.
/// `top_level_function` marks exactly the top-level `FunctionDeclaration`s the
/// reachability skip in [`TypeChecker::pin_result_parameters`] applies to — a method
/// body and a top-level binding's value are always reachable roots.
struct NestedDeclaration<'a> {
    name: String,
    parameters: &'a [Parameter],
    declared: Option<&'a [Type]>,
    body: &'a Expression,
    owning_type: Option<String>,
    top_level_function: bool,
}

/// Every function, lambda, and method declared anywhere in `program`, top-level or
/// nested at any depth — each its own candidate for
/// [`TypeChecker::pin_result_parameters`], checked against its own parameters and body
/// only.
fn declaration_candidates(program: &Program) -> Vec<NestedDeclaration<'_>> {
    let mut candidates = Vec::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => {
                if declaration.is_inert_corelib_placeholder() {
                    continue;
                }
                candidates.push(NestedDeclaration {
                    name: declaration.name.clone(),
                    parameters: &declaration.parameters,
                    declared: declaration.declared_parameters(),
                    body: &declaration.body,
                    owning_type: None,
                    top_level_function: true,
                });
                candidates.extend(nested_declaration_candidates(&declaration.body));
            }
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    candidates.push(NestedDeclaration {
                        name: method.name.clone(),
                        parameters: &method.parameters,
                        declared: None,
                        body: &method.body,
                        owning_type: Some(declaration.name.clone()),
                        top_level_function: false,
                    });
                    candidates.extend(nested_declaration_candidates(&method.body));
                }
            }
            Item::VariableDeclaration(declaration) => match &declaration.value {
                // Named directly by its binding, rather than through the generic
                // `Expression::Lambda` walk below (which would also find this same
                // lambda, unhelpfully labeled "a lambda").
                Expression::Lambda {
                    parameters, body, ..
                } => {
                    candidates.push(NestedDeclaration {
                        name: declaration.name.clone(),
                        parameters,
                        declared: None,
                        body,
                        owning_type: None,
                        top_level_function: false,
                    });
                    candidates.extend(nested_declaration_candidates(body));
                }
                other => candidates.extend(nested_declaration_candidates(other)),
            },
            // A trap arm has no declared parameter list of its own to pin.
            Item::TrapDeclaration(trap) => {
                for arm in &trap.arms {
                    candidates.extend(nested_declaration_candidates(&arm.body));
                }
            }
        }
    }
    candidates
}

/// Every function/lambda/method-shaped construct anywhere inside `body`, at any nesting
/// depth — `try_for_each_subexpression` already descends into a nested declaration's or
/// bound lambda's own body, so one walk finds every depth. An anonymous lambda literal
/// has no name of its own, so `"a lambda"` names it instead.
fn nested_declaration_candidates(body: &Expression) -> Vec<NestedDeclaration<'_>> {
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
                owning_type: None,
                top_level_function: false,
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
                                owning_type: None,
                                top_level_function: false,
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
                                owning_type: None,
                                top_level_function: false,
                            });
                        }
                        // A type declared locally has methods exactly like a top-level
                        // one's — each is its own candidate.
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                candidates.push(NestedDeclaration {
                                    name: method.name.clone(),
                                    parameters: &method.parameters,
                                    declared: None,
                                    body: &method.body,
                                    owning_type: Some(declaration.name.clone()),
                                    top_level_function: false,
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
struct MatchCandidate<'a> {
    scrutinee_name: String,
    scrutinee_span: Span,
    ok_binding: Option<(String, Span, &'a Expression)>,
    not_ok_binding: Option<(String, Span, &'a Expression)>,
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
fn result_parameter_matches<'a>(
    type_table: &TypeTable,
    body: &'a Expression,
    parameter_name: &str,
) -> Vec<MatchCandidate<'a>> {
    use crate::ast::{NOT_OK, OK};

    // The parameter itself is always its own alias, everywhere in its own body.
    let mut entries = vec![AliasEntry {
        name: parameter_name.to_string(),
        scope: body.span().clone(),
        alive: true,
    }];
    let mut candidates: Vec<MatchCandidate<'a>> = Vec::new();

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
                            let binding = (binding_name.clone(), binding_span.clone(), &arm.body);
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
        .collect()
}

/// A one-time index over the whole program, shared by every candidate parameter
/// [`TypeChecker::pin_result_parameters`] considers (rather than re-walking the program
/// once per candidate): `calls_by_name[callee]` is every plain (non-member) call
/// expression naming `callee` — the only signal [`TypeChecker::pin_one_result_parameter`]
/// draws from, since a variant no such call demonstrates has no payload type regardless
/// of whether the declaration is referenced some OTHER way.
fn index_program_calls(program: &Program) -> HashMap<&str, Vec<&Expression>> {
    let mut calls_by_name: HashMap<&str, Vec<&Expression>> = HashMap::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => {
                index_expression(&declaration.body, &mut calls_by_name)
            }
            Item::VariableDeclaration(declaration) => {
                index_expression(&declaration.value, &mut calls_by_name)
            }
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    index_expression(&method.body, &mut calls_by_name);
                }
            }
            Item::TrapDeclaration(trap) => {
                for arm in &trap.arms {
                    index_expression(&arm.body, &mut calls_by_name);
                }
            }
        }
    }
    calls_by_name
}

/// [`index_program_calls`]'s per-expression work — its own named, explicitly-lifetimed
/// function rather than a closure, which a plain `impl FnMut` cannot express here: the
/// borrowed `Expression`s this map collects must outlive the call that finds them.
fn index_expression<'a>(
    expression: &'a Expression,
    calls_by_name: &mut HashMap<&'a str, Vec<&'a Expression>>,
) {
    let _: ControlFlow<()> = try_for_each_subexpression(expression, &mut |e| {
        if let Expression::Call {
            function,
            member_call: false,
            ..
        } = e
            && let Expression::Identifier { name, .. } = function.as_ref()
        {
            calls_by_name.entry(name.as_str()).or_default().push(e);
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

/// Whether `name` — a match arm's own bound payload — is READ anywhere within
/// `arm_body`: any occurrence of it as a bare identifier, other than one shadowed by a
/// nested function/lambda parameter, a local declaration, or a nested match arm's own
/// pattern reusing the same name (each of which then refers to something else, not this
/// binding, for the rest of that shadow's own scope). This is the ONLY thing
/// [`TypeChecker::pin_one_result_parameter`] needs to know about an unpinned variant:
/// with no caller ever demonstrating its type, a REAL read has no representation to
/// materialize, while a bound-but-unread payload (`Ok(x) => 0`, never touching `x`)
/// needs no type at all — nothing observes it. A purely local scan, like
/// [`result_parameter_matches`] itself: it reads only `arm_body`, never another
/// declaration's.
fn is_payload_read(arm_body: &Expression, name: &str) -> bool {
    let mut occurrences: Vec<Span> = Vec::new();
    let mut shadows: Vec<Span> = Vec::new();
    let _: ControlFlow<()> = try_for_each_subexpression(arm_body, &mut |expression| {
        match expression {
            Expression::Identifier { name: n, span } if n == name => occurrences.push(span.clone()),
            Expression::Lambda {
                parameters, span, ..
            } if parameters.iter().any(|p| p.name == name) => {
                shadows.push(span.clone());
            }
            Expression::Match { arms, .. } => {
                for arm in arms {
                    if pattern_binds(&arm.pattern, name) {
                        shadows.push(arm.body.span().clone());
                    }
                }
            }
            Expression::Block { statements, span } => {
                for statement in statements {
                    match statement {
                        Statement::Item(Item::FunctionDeclaration(nested)) => {
                            if nested.parameters.iter().any(|p| p.name == name) {
                                shadows.push(nested.span.clone());
                            }
                            if nested.name == name {
                                shadows.push(Span {
                                    start: nested.span.start,
                                    end: span.end,
                                    file: span.file,
                                });
                            }
                        }
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                if method.parameters.iter().any(|p| p.name == name) {
                                    shadows.push(method.span.clone());
                                }
                            }
                        }
                        // A local declaration reusing `name` shadows it for the REST of
                        // this block, starting right after its OWN value expression —
                        // never from the declaration's own start, which would wrongly
                        // hide a self-referential read (`text := text + "!"`) inside
                        // that very value.
                        Statement::Item(Item::VariableDeclaration(local)) if local.name == name => {
                            shadows.push(Span {
                                start: local.value.span().end,
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
    occurrences.iter().any(|occurrence| {
        !shadows.iter().any(|shadow| {
            shadow.file == occurrence.file
                && shadow.start <= occurrence.start
                && occurrence.end <= shadow.end
        })
    })
}

/// Whether `pattern` binds `name` — a constructor sub-pattern's own identifier, or a
/// bare identifier pattern — used by [`is_payload_read`] to tell a nested match arm that
/// rebinds the same name (shadowing it) from one that doesn't.
fn pattern_binds(pattern: &Pattern, name: &str) -> bool {
    match pattern {
        Pattern::Identifier { name: n, .. } => n == name,
        Pattern::Constructor { arguments, .. } => arguments
            .iter()
            .any(|argument| pattern_binds(argument, name)),
        Pattern::Wildcard { .. } | Pattern::Number { .. } | Pattern::Text { .. } => false,
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
