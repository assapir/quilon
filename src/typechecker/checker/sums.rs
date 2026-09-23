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
    /// is a `TypeMismatch` at the second one.
    ///
    /// A variant NO caller demonstrates has no payload type at all — full stop, whether
    /// the declaration is called elsewhere without informing that specific variant, or
    /// never called at all. A match arm that binds AND READS that payload — any use of
    /// the bound name, anywhere in the arm's own body, honoring shadowing (see
    /// [`is_payload_read`]) — is `UnresolvedResultPayload`, naming the function, the
    /// parameter, and the unresolved variant. Binding it WITHOUT reading it (`Ok(x) =>
    /// 0`, never touching `x`) or discarding it outright (`Ok(_)`) needs no payload type
    /// at all and is accepted either way — nothing ever observes its representation, so
    /// a function dispatched on the tag alone (`okTag`-style, every call passing a
    /// different concrete payload) is untouched.
    ///
    /// A method's or an overload member's own parameter is pinned from its direct
    /// callers too — the checker already resolves a member call to its receiver's type
    /// and an overloaded call to one member by argument types (`check_call`,
    /// `resolve_overload`), so each records its OWN call's argument spans at that same
    /// point (`TypeChecker::method_call_args`/`overload_call_args`) rather than through
    /// a name-keyed scan, which could never attribute a member call to one
    /// receiver-independent signature or an overloaded call to one member by name alone
    /// (see [`CallOrigin`]).
    ///
    /// This design is narrower than it once was: pinning a plain function's or a bound
    /// lambda's parameter from a whole-program, name-keyed scan of call sites went
    /// through several review rounds, each finding a new way it mis-attributes a match
    /// to the wrong declaration (a forwarding wrapper's own uninformative call, a nested
    /// function or lambda the scan never visited, a local rebinding that shadows the
    /// outer name) — all fixed below, by scoping every match/shadow check to purely
    /// LOCAL, per-declaration information (nothing here reads what ANOTHER
    /// declaration's body contains) and reserving the whole-program scan strictly for
    /// reading a DIRECT call's own, already-checked argument type.
    ///
    /// A `Result`'s payload type crossing a function boundary through its RETURN,
    /// including an OVERLOADED function's, is a different, already-sound mechanism —
    /// see [`Self::refine_overload_return_type`] and `check_function_declaration`'s
    /// own-`env`-binding refinement — since a function's return is pinned from its OWN
    /// body, never from a caller.
    ///
    /// A top-level function nothing reachable from `^` calls is never emitted —
    /// [`crate::ast::reachability::reachable_functions`] is the same tree-shaking
    /// analysis `generator.rs` already skips it by, most visibly a helper only called
    /// from inside a `test.describe`/`test.it` block, which `run`/`check`/`build` erase
    /// entirely (`quilon test` synthesizes its own `^` that DOES reach it, so it IS
    /// checked there). This check is skipped for such a function entirely, not merely
    /// relaxed: a program codegen would happily compile once the dead code is gone must
    /// not fail here first. A method body and every top-level binding's value are
    /// always reachable roots (`reachable_functions` never prunes them), so only a
    /// plain top-level function is subject to this skip; `reachable_functions`
    /// returning `None` means there is no `^` to measure reachability from, so nothing
    /// is skipped — matching codegen, which then keeps everything too.
    ///
    /// Rejecting every unpinned-but-read variant here is what lets codegen stop
    /// defaulting an undetermined payload's representation to `Num` at all: a program
    /// this pass accepts never reaches `oracle::value_repr_type`'s `Type::Generic` arm
    /// with a payload anything actually reads, so that arm is now an internal error
    /// (see its own doc comment) rather than a silent `f64` fallback.
    pub(super) fn pin_result_parameters(&mut self, program: &Program) -> Result<(), TypeError> {
        let calls_by_name = index_program_calls(program);
        let reachable = crate::ast::reachability::reachable_functions(program);

        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) => {
                    if declaration.is_inert_corelib_placeholder() {
                        continue;
                    }
                    if !reachable
                        .as_ref()
                        .is_none_or(|reachable| reachable.contains(declaration.name.as_str()))
                    {
                        continue;
                    }
                    let origin = if self.overloaded_names.contains(&declaration.name) {
                        CallOrigin::Overload {
                            name: declaration.name.clone(),
                            parameter_types: self.resolved_parameter_types(
                                &declaration.parameters,
                                declaration.declared_parameters(),
                            ),
                        }
                    } else {
                        CallOrigin::Named(declaration.name.clone())
                    };
                    self.pin_result_parameters_of(
                        &declaration.name,
                        &declaration.parameters,
                        declaration.declared_parameters(),
                        &declaration.body,
                        &origin,
                        &calls_by_name,
                    )?;
                    self.pin_nested_result_parameters(&declaration.body, &calls_by_name)?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        // A method has no whole-signature `::` form of its own (no
                        // `binding_type`) — every parameter is annotated directly, or
                        // `check_type_methods` already rejected the declaration.
                        let origin = self.method_call_origin(
                            &declaration.name,
                            &method.name,
                            &method.parameters,
                        );
                        self.pin_result_parameters_of(
                            &method.name,
                            &method.parameters,
                            None,
                            &method.body,
                            &origin,
                            &calls_by_name,
                        )?;
                        self.pin_nested_result_parameters(&method.body, &calls_by_name)?;
                    }
                }
                Item::VariableDeclaration(declaration) => match &declaration.value {
                    // Checked directly, by its real binding name, rather than through
                    // `pin_nested_result_parameters`'s generic walk (which would also
                    // find this same top-level lambda, unhelpfully labeled "a lambda") —
                    // its own body is still handed to that walk, for anything nested
                    // further in. A lambda literal has no whole-signature form either
                    // (no `binding_type`): its parameters take their types only from
                    // their own annotations. Called by its binding's name exactly like a
                    // `FunctionDeclaration`, and never overloaded.
                    Expression::Lambda {
                        parameters, body, ..
                    } => {
                        self.pin_result_parameters_of(
                            &declaration.name,
                            parameters,
                            None,
                            body,
                            &CallOrigin::Named(declaration.name.clone()),
                            &calls_by_name,
                        )?;
                        self.pin_nested_result_parameters(body, &calls_by_name)?;
                    }
                    other => self.pin_nested_result_parameters(other, &calls_by_name)?,
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
    /// declaration's body contains). A nested function or lambda is called by NAME (its
    /// name is never in `overloaded_names`, which only a top-level `FunctionDeclaration`
    /// joins, and never a method either); a nested type's method is resolved the same
    /// way a top-level one's is (see [`Self::method_call_origin`]).
    fn pin_nested_result_parameters(
        &mut self,
        body: &Expression,
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
    ) -> Result<(), TypeError> {
        for candidate in nested_function_candidates(body) {
            let origin = match &candidate.owning_type {
                Some(owning_type) => {
                    self.method_call_origin(owning_type, &candidate.name, candidate.parameters)
                }
                None => CallOrigin::Named(candidate.name.clone()),
            };
            self.pin_result_parameters_of(
                &candidate.name,
                candidate.parameters,
                candidate.declared,
                candidate.body,
                &origin,
                calls_by_name,
            )?;
        }
        Ok(())
    }

    /// The resolved types of `parameters` (or `declared`'s whole-signature slots where a
    /// parameter has no annotation of its own), in order — an overload member's own key
    /// into `overloads`/`overload_call_args`, computed the same way
    /// `register_overload_declaration` computes it for registration.
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

    /// Where a method named `method_name` on `owning_type`'s own calls come from: a
    /// qualified `"Type.method"` overload set (2+ methods of this name on `owning_type`)
    /// resolves exactly like a top-level overload does, so its calls are attributed to
    /// it the same way (`CallOrigin::Overload`); otherwise a member call is attributed
    /// to it by `owning_type` and its own name alone (`CallOrigin::Method`).
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

    /// [`Self::pin_result_parameters`]'s per-declaration work, shared by a top-level
    /// function, a method, a lambda, and every nested candidate: every bare `:: Result`
    /// parameter of `parameters` that `body` matches directly. `declared` is a
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

    /// The argument spans of every direct call [`CallOrigin`] attributes to a
    /// declaration — a plain function's or a bound lambda's found by NAME in the
    /// whole-program `calls_by_name` index (computed once, up front); a method's or an
    /// overload member's own calls were recorded as `check_call`/`resolve_overload`
    /// resolved each one, which this only reads back.
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

    /// [`Self::pin_result_parameters_of`]'s per-parameter work: fold every direct
    /// caller's argument at `index` into a pinned `Ok`/`NotOk` payload. A variant that
    /// ends up unpinned but is bound AND READ somewhere in `sites` is
    /// `UnresolvedResultPayload`; otherwise, whatever WAS pinned is taught to every
    /// matched site.
    fn pin_one_result_parameter(
        &mut self,
        name: &str,
        index: usize,
        parameter_name: &str,
        sites: &[ResultParameterMatch<'_>],
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

/// Where a declaration's own direct callers' arguments come from — the three shapes
/// [`TypeChecker::pin_one_result_parameter`] draws from, each attributing a call to its
/// callee a different way: a plain function or a bound lambda by NAME (looked up in the
/// whole-program `calls_by_name` index, computed once up front); a method by its
/// RECEIVER's type (recorded by `check_call` as it resolves one, since a member call
/// can't be attributed to a callee by name alone); an overload member — top-level, or a
/// type's own qualified `"Type.method"` set — by its OWN declared parameter types
/// (recorded by `resolve_overload` as it resolves one to exactly one member, since
/// several members share the overloaded name).
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

/// A function, lambda, or method declared somewhere inside another declaration's body —
/// [`TypeChecker::pin_nested_result_parameters`]'s own candidate to check next, against
/// its own parameters and its own body only. `declared` is its whole-signature `::`
/// annotation's parameter slots, when it has one (only a nested `FunctionDeclaration`
/// can; a lambda literal or a method, named or not, never does). `owning_type` is the
/// enclosing type's own name for a locally-declared type's method, and `None` for
/// everything else — resolved the same way a top-level method's calls are (see
/// [`TypeChecker::method_call_origin`]), never by a plain name lookup.
struct NestedDeclaration<'a> {
    name: String,
    parameters: &'a [Parameter],
    declared: Option<&'a [Type]>,
    body: &'a Expression,
    owning_type: Option<String>,
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
                owning_type: None,
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
                            });
                        }
                        // A type declared locally (inside a function's own body) has
                        // methods exactly like a top-level one's — each is its own
                        // candidate, resolved the same way (see
                        // `TypeChecker::method_call_origin`).
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                candidates.push(NestedDeclaration {
                                    name: method.name.clone(),
                                    parameters: &method.parameters,
                                    declared: None,
                                    body: &method.body,
                                    owning_type: Some(declaration.name.clone()),
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
/// for. `ok_binding`/`not_ok_binding` are the payload sub-pattern's own name, span, and
/// ARM BODY, when that arm binds a name (`Ok(text)`) rather than discarding the payload
/// (`Ok(_)`) — the body is what [`is_payload_read`] checks for a use of that name.
/// Matching a DERIVED value (a call, a field, a renamed local's further transformation)
/// is out of reach for this pass — matching the producing call directly, or annotating,
/// still works.
struct ResultParameterMatch<'a> {
    scrutinee_span: Span,
    ok_binding: Option<(String, Span, &'a Expression)>,
    not_ok_binding: Option<(String, Span, &'a Expression)>,
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
) -> Vec<ResultParameterMatch<'a>> {
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
