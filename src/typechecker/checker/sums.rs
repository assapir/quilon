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

    /// After the whole program is checked, pin the payload type of a `Result`-typed
    /// PARAMETER — of a plain top-level function, or of a record/sum's method — that
    /// stays the raw, unspecialized built-in type through its own declaration
    /// (`resolve_type` gives every bare `:: Result` the same generic `Ok(T)`/`NotOk(E)`
    /// shape) but is matched directly in its body, binding a payload.
    ///
    /// Only a PLAIN (non-overloaded) top-level function's parameter is actually pinned:
    /// its callers are DIRECT calls to one bare name, so an argument's already-checked
    /// oracle type is the same source [`Self::check_constructor_call`] draws from for a
    /// constructor's own result. An overload set's members share that one bare name — a
    /// call to it doesn't say which member's parameter the argument fills — and a
    /// method's calls are member calls (`recv.name(...)`), which carry no
    /// receiver-type-independent call site to scan; pinning either soundly would need
    /// resolving which member/receiver a call reaches, which this whole-program,
    /// name-keyed pass does not attempt. For both, a bound payload is
    /// `UnresolvedResultPayload` unconditionally, never silently defaulted — the
    /// overloaded-RETURN equivalent of this gap is closed instead by
    /// [`Self::refine_overload_return_type`], which runs per member as its own body is
    /// checked and needs no cross-call scan.
    ///
    /// For a pinnable parameter: two DIRECT callers disagreeing over a position's payload
    /// is a `TypeMismatch` at the second one. A bound position no direct call ever
    /// informs is `UnresolvedResultPayload` — UNLESS the function's name is never written
    /// as a direct call anywhere but IS referenced some other way (passed to `.map`,
    /// assigned to a binding): a real caller may well reach it through a shape this pass
    /// cannot see, so the position is left `Generic` rather than rejecting working code
    /// (the same risk the historical default always carried for such a call, not a new
    /// one this pass introduces). A position no site ever binds (`Ok(_)`) needs no
    /// payload type at all, so a function dispatched on the tag alone (`okTag`-style,
    /// every call passing a different concrete payload) is untouched.
    pub(super) fn pin_result_parameters(&mut self, program: &Program) -> Result<(), TypeError> {
        let (calls_by_name, referenced_names) = index_program_calls(program);

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
                        &declaration.body,
                        pinnable,
                        &calls_by_name,
                        &referenced_names,
                    )?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        self.pin_result_parameters_of(
                            &method.name,
                            &method.parameters,
                            &method.body,
                            false,
                            &calls_by_name,
                            &referenced_names,
                        )?;
                    }
                }
                // A `:=`/`=`-bound lambda, called by its binding's name exactly like a
                // `FunctionDeclaration` (`calls.rs`'s plain-call path resolves either the
                // same way), is just as pinnable — and never overloaded (only a
                // `FunctionDeclaration`'s name joins `overloaded_names`).
                Item::VariableDeclaration(declaration) => {
                    if let Expression::Lambda {
                        parameters, body, ..
                    } = &declaration.value
                    {
                        self.pin_result_parameters_of(
                            &declaration.name,
                            parameters,
                            body,
                            true,
                            &calls_by_name,
                            &referenced_names,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// [`Self::pin_result_parameters`]'s per-declaration work, shared by a top-level
    /// function and a method: every bare `:: Result` parameter of `parameters` that
    /// `body` matches directly. `pinnable` is false for a method or an overload member,
    /// where a bound payload is reported unconditionally rather than pinning attempted
    /// (see the doc comment above).
    fn pin_result_parameters_of(
        &mut self,
        name: &str,
        parameters: &[Parameter],
        body: &Expression,
        pinnable: bool,
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
        referenced_names: &HashSet<&str>,
    ) -> Result<(), TypeError> {
        for (index, parameter) in parameters.iter().enumerate() {
            let Some(annotation) = &parameter.type_annotation else {
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
            self.pin_one_result_parameter(
                name,
                index,
                &parameter.name,
                &sites,
                calls_by_name,
                referenced_names,
            )?;
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
        calls_by_name: &HashMap<&str, Vec<&Expression>>,
        referenced_names: &HashSet<&str>,
    ) -> Result<(), TypeError> {
        use crate::ast::{NOT_OK, OK, RESULT_TYPE_NAME, SumVariant};

        // Only a variant some site actually BINDS needs a single, agreed-on payload type —
        // a discarded payload (`Ok(_)`) never reads its value, so callers may legitimately
        // pass it different concrete types at every call (`okTag`-style dispatch on the tag
        // alone). Unifying a position nothing binds would reject that as a false conflict.
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

        // A direct call exists (this position just never saw a concrete argument through
        // it, e.g. it only ever forwards an equally-unpinned value one hop further) —
        // genuinely unresolved, reported below. No direct call exists at ALL, but the
        // name is referenced some other way — leniently leave it generic instead: this
        // pass cannot rule out a real caller reaching it through a shape it does not scan
        // (a first-class reference, an alias), and rejecting would be a false positive on
        // working code.
        let lenient = !calls_by_name.contains_key(name) && referenced_names.contains(name);
        if !lenient {
            for site in sites {
                if let Some(span) = &site.ok_binding
                    && ok_pin.is_none()
                {
                    return Err(TypeError::UnresolvedResultPayload {
                        function: name.to_string(),
                        parameter: parameter_name.to_string(),
                        span: span.clone(),
                    });
                }
                if let Some(span) = &site.not_ok_binding
                    && not_ok_pin.is_none()
                {
                    return Err(TypeError::UnresolvedResultPayload {
                        function: name.to_string(),
                        parameter: parameter_name.to_string(),
                        span: span.clone(),
                    });
                }
            }
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

/// One `?`/`|` match, within a declaration's body, whose SCRUTINEE is a bare read of one
/// of its own `Result`-typed parameters — what [`TypeChecker::pin_result_parameters`]
/// looks for. `ok_binding`/`not_ok_binding` are the payload sub-pattern's own span, when
/// that arm binds a name (`Ok(text)`) rather than discarding the payload (`Ok(_)`);
/// matching a DERIVED value (a call, a field, a renamed local) is out of reach for this
/// pass — matching the producing call directly, or annotating, still works.
struct ResultParameterMatch {
    scrutinee_span: Span,
    ok_binding: Option<Span>,
    not_ok_binding: Option<Span>,
}

/// The span of the first payload binding (`ok_binding` or `not_ok_binding`) among
/// `sites`, if any binds one at all — the span an unpinnable declaration's
/// `UnresolvedResultPayload` names.
fn first_binding(sites: &[ResultParameterMatch]) -> Option<&Span> {
    sites
        .iter()
        .find_map(|site| site.ok_binding.as_ref().or(site.not_ok_binding.as_ref()))
}

/// Every `Match` expression in `body` whose scrutinee is a bare read of `parameter_name`,
/// EXCLUDING one that sits inside a nested scope that rebinds that same name — a nested
/// function's or lambda's own same-named parameter, or a local `=`/`:=`/nested-function
/// declaration reusing the name from partway through the enclosing block onward. Past
/// that point the name refers to a different value entirely, and treating its match as
/// this parameter's own would teach the wrong binding a type pinned from someone else's
/// callers (each shadowing declaration's own region, gathered in the same walk, is
/// excluded by byte range — cheaper than threading scope state through every expression
/// variant, since nothing outside a scope can read a name it shadows).
///
/// A SECOND, independent guard catches what a byte-range region cannot: a shadow whose
/// own value is not a `Result` at all never reaches here (matching `Ok`/`NotOk` against
/// it is rejected earlier, at the first checking pass, as a constructor pattern on a
/// non-sum scrutinee) — but a shadow that reassigns the name to an ALREADY-CONCRETE
/// `Result` (`result := Ok(aNum)`, say) type-checks fine and reaches this scan looking
/// identical, by name, to a genuine read of the still-generic parameter. `type_table`,
/// the checker's own first-pass record, disambiguates: it holds each identifier
/// occurrence's REAL type from before this pass touches anything, so a site is kept only
/// when its scrutinee's own recorded type is exactly as unspecialized as the target
/// parameter's — a reassignment to something already concrete recorded something else
/// and is dropped. (A reassignment to another value that HAPPENS to still be an
/// unspecialized `Result`, e.g. forwarded from a different unpinned parameter, is not
/// caught by this guard — narrower than the byte-range one above, and, like a
/// same-named nested parameter, a rarer collision this pass leaves to the region guard.)
fn result_parameter_matches(
    type_table: &TypeTable,
    body: &Expression,
    parameter_name: &str,
) -> Vec<ResultParameterMatch> {
    use crate::ast::{NOT_OK, OK};

    let mut sites = Vec::new();
    let mut shadows: Vec<Span> = Vec::new();
    let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
        match expression {
            Expression::Match {
                expression: scrutinee,
                arms,
                ..
            } if matches!(
                scrutinee.as_ref(),
                Expression::Identifier { name, .. } if name == parameter_name
            ) =>
            {
                let Expression::Identifier { span, .. } = scrutinee.as_ref() else {
                    unreachable!("matched above")
                };
                if !type_table.get(span).is_some_and(is_unspecialized_result) {
                    return ControlFlow::Continue(());
                }
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
            Expression::Lambda {
                parameters, span, ..
            } if parameters.iter().any(|p| p.name == parameter_name) => {
                shadows.push(span.clone());
            }
            Expression::Block { statements, span } => {
                for statement in statements {
                    match statement {
                        // A nested function's own PARAMETER of this name shadows it for
                        // that function's whole span (its body, but also the function
                        // literal itself — nothing outside can reach in either way).
                        Statement::Item(Item::FunctionDeclaration(nested))
                            if nested.parameters.iter().any(|p| p.name == parameter_name) =>
                        {
                            shadows.push(nested.span.clone());
                        }
                        // A nested function's or a local `=`/`:=` binding's own NAME
                        // rebinds it from that declaration onward, through the rest of
                        // THIS block (statements execute in the order written, and nest
                        // no further scope of their own).
                        Statement::Item(Item::FunctionDeclaration(nested))
                            if nested.name == parameter_name =>
                        {
                            shadows.push(Span {
                                start: nested.span.start,
                                end: span.end,
                                file: span.file,
                            });
                        }
                        Statement::Item(Item::VariableDeclaration(local))
                            if local.name == parameter_name =>
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

    sites.retain(|site| {
        !shadows.iter().any(|shadow| {
            shadow.file == site.scrutinee_span.file
                && shadow.start <= site.scrutinee_span.start
                && site.scrutinee_span.end <= shadow.end
        })
    });
    sites
}

/// A one-time index over the whole program, shared by every candidate parameter
/// [`TypeChecker::pin_result_parameters`] considers (rather than re-walking the program
/// once per candidate): `calls_by_name[callee]` is every plain (non-member) call
/// expression naming `callee`, and `referenced_names` is every name any `Identifier`
/// expression anywhere reads (a superset of the callees above, since a call's own
/// function position is itself an `Identifier`) — the signal
/// [`TypeChecker::pin_one_result_parameter`] uses to tell "no direct call, and nothing
/// else reaches this name either" (genuinely unresolved) from "no direct call, but the
/// name is referenced some other way" (leniently left generic — a real caller may reach
/// it through a shape this pass does not scan).
fn index_program_calls(program: &Program) -> (HashMap<&str, Vec<&Expression>>, HashSet<&str>) {
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
    (calls_by_name, referenced_names)
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
