//! Overload sets: registering each member's signature and resolving a call or operator
//! to exactly one of them by argument type.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;

impl TypeChecker {
    /// Register the built-in operator overloads so the standard operators dispatch
    /// through the SAME exact-match mechanism as user overloads — `+` on `Num` and
    /// `+` on `Text` (concat) are just two members of the `+` overload set, etc.
    pub(super) fn add_builtin_overloads(&mut self) {
        let arith = [
            BinaryOperator::Add,
            BinaryOperator::Sub,
            BinaryOperator::Mul,
            BinaryOperator::Div,
            BinaryOperator::Mod,
        ];
        for operator in arith {
            // Num operator Num -> Num.
            self.add_overload(
                operator.symbol(),
                Overload {
                    parameters: vec![Type::Num, Type::Num],
                    ret: Some(Type::Num),
                    result_aliasing: Some(ResultAliasing::default()),
                    is_static: false,
                },
            );
        }
        // `+` also concatenates Text.
        self.add_overload(
            BinaryOperator::Add.symbol(),
            Overload {
                parameters: vec![Type::Text, Type::Text],
                ret: Some(Type::Text),
                result_aliasing: Some(ResultAliasing::default()),
                is_static: false,
            },
        );

        // Comparisons. Equality (`==`/`!=`) over every built-in scalar; ordering
        // (`<`/`<=`/`>`/`>=`) over Num and Text (Text is lexicographic — the
        // concrete deliverable). All yield Bool.
        let eq_ops = [BinaryOperator::Eq, BinaryOperator::Ne];
        let ord_ops = [
            BinaryOperator::Lt,
            BinaryOperator::Le,
            BinaryOperator::Gt,
            BinaryOperator::Ge,
        ];
        for operator in eq_ops {
            for ty in [Type::Num, Type::Text, Type::Bool] {
                self.add_overload(
                    operator.symbol(),
                    Overload {
                        parameters: vec![ty.clone(), ty],
                        ret: Some(Type::Bool),
                        result_aliasing: Some(ResultAliasing::default()),
                        is_static: false,
                    },
                );
            }
        }
        for operator in ord_ops {
            for ty in [Type::Num, Type::Text] {
                self.add_overload(
                    operator.symbol(),
                    Overload {
                        parameters: vec![ty.clone(), ty],
                        ret: Some(Type::Bool),
                        result_aliasing: Some(ResultAliasing::default()),
                        is_static: false,
                    },
                );
            }
        }

        // Logical `&&`/`||`: Bool operator Bool -> Bool.
        for operator in [BinaryOperator::And, BinaryOperator::Or] {
            self.add_overload(
                operator.symbol(),
                Overload {
                    parameters: vec![Type::Bool, Type::Bool],
                    ret: Some(Type::Bool),
                    result_aliasing: Some(ResultAliasing::default()),
                    is_static: false,
                },
            );
        }

        // The functions the compiler provides itself — `core.time.now` and the internal
        // `__` primitives — as members of their own sets, from the one table codegen also
        // dispatches and mangles by. Only the bare `__` names can meet a user definition
        // (which then adds a member beside the intrinsic); the module-qualified ones are
        // names no user file can declare. (The output built-ins take no signature per
        // type; see `check_renderable_builtin_call`.)
        for member in crate::ast::BUILTIN_OVERLOADS {
            self.add_overload(
                member.name,
                Overload {
                    parameters: member.parameters.to_vec(),
                    ret: Some(member.ret.clone()),
                    result_aliasing: Some(ResultAliasing::default()),
                    is_static: false,
                },
            );
        }
    }

    /// Add one member to the overload set `name`.
    pub(super) fn add_overload(&mut self, name: &str, overload: Overload) {
        self.overloads
            .entry(name.to_string())
            .or_default()
            .push(overload);
    }

    /// Whether overload set `name` has a member whose parameters EXACTLY match `arg_types`
    /// (no coercion) — a non-erroring probe, used to ask whether a type carries an operator
    /// member (`==`, the `%` hash hook) without raising a dispatch error. A member whose
    /// LAST parameter is the built-in `Site` also matches one argument short of it: that
    /// argument is the caller's location, which the compiler fills in.
    pub(super) fn has_exact_overload(&self, name: &str, arg_types: &[Type]) -> bool {
        self.overloads.get(name).is_some_and(|set| {
            set.iter()
                .any(|o| crate::ast::parameters_accept(&o.parameters, arg_types, types_match))
        })
    }

    /// Check a call to overload set `name`. The arguments that are NOT lambdas are typed
    /// first — they are what narrows the set — and each lambda argument is then typed
    /// against the parameter every surviving candidate agrees on, so `record(1, (n) => n)`
    /// works whenever the rest of the call picks the member out. Where the candidates
    /// disagree the target type is unknown, and the lambda must annotate its parameters;
    /// the error then says which set was left open.
    pub(super) fn check_overloaded_call(
        &mut self,
        name: &str,
        arguments: &[Expression],
        first_ty: Option<&Type>,
        span: &Span,
    ) -> Result<Type, TypeError> {
        let mut arg_types: Vec<Option<Type>> = Vec::with_capacity(arguments.len());
        for (i, argument) in arguments.iter().enumerate() {
            arg_types.push(match (i, first_ty, argument) {
                (0, Some(ty), _) => Some(ty.clone()),
                (_, _, Expression::Lambda { .. }) => None,
                _ => Some(self.infer_expression(argument)?),
            });
        }

        for (i, argument) in arguments.iter().enumerate() {
            if arg_types[i].is_some() {
                continue;
            }
            let narrowed = self.narrowed_parameter(name, &arg_types, i);
            let target = match &narrowed {
                Some(ty) => LambdaTarget::Declared(ty),
                None => LambdaTarget::OpenOverload(name),
            };
            arg_types[i] = Some(self.infer_argument(argument, target)?);
        }

        let arg_types: Vec<Type> = arg_types.into_iter().flatten().collect();
        let arg_spans: Vec<Span> = arguments.iter().map(|a| a.span().clone()).collect();
        self.resolve_overload(name, &arg_types, &arg_spans, span)
    }

    /// The type every still-possible member of `name` gives the argument at `index` — the
    /// target a lambda there is typed against. A member is still possible when its arity
    /// fits and every already-typed argument matches it exactly; `None` when the survivors
    /// disagree (or none is left), which is precisely when the position states no type.
    fn narrowed_parameter(
        &self,
        name: &str,
        arg_types: &[Option<Type>],
        index: usize,
    ) -> Option<Type> {
        let mut narrowed: Option<&Type> = None;
        for overload in self.overloads.get(name)? {
            // An argument still to be typed matches anything: it is what the surviving
            // candidates are being asked about.
            let possible =
                crate::ast::parameters_accept(&overload.parameters, arg_types, |p, a| {
                    a.as_ref().is_none_or(|a| types_match(p, a))
                });
            if !possible {
                continue;
            }
            match narrowed {
                Some(previous) if previous != &overload.parameters[index] => return None,
                _ => narrowed = Some(&overload.parameters[index]),
            }
        }
        narrowed.cloned()
    }

    /// Resolve a call to overload set `name` by EXACT argument-type match (no implicit
    /// coercion). Returns the matched overload's return type. Errors on no match or
    /// (with exact matching, a duplicate-signature) ambiguity, listing the candidates.
    pub(super) fn resolve_overload(
        &self,
        name: &str,
        arg_types: &[Type],
        arg_spans: &[Span],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let set = self.overloads.get(name);
        let matches: Vec<&Overload> = set
            .map(|s| {
                s.iter()
                    .filter(|o| {
                        crate::ast::parameters_accept(&o.parameters, arg_types, types_match)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Candidate signatures are only needed to render an error, so build them lazily.
        let candidates = || -> Vec<Vec<Type>> {
            set.map(|s| s.iter().map(|o| o.parameters.clone()).collect())
                .unwrap_or_default()
        };

        match matches.as_slice() {
            [] => Err(TypeError::NoMatchingOverload {
                name: name.to_string(),
                arg_types: arg_types.to_vec(),
                arg_spans: arg_spans.to_vec(),
                candidates: candidates(),
                span: span.clone(),
            }),
            // Re-resolve the result type: an overloaded member's return annotation may
            // have been registered (pre-pass) before its named type existed, so a bare
            // `Named{T, fields:[]}` is filled in to its full definition here. A member
            // with no return annotation has no result type to give this call.
            [only] => match &only.ret {
                Some(ret) => Ok(self.resolve_type(ret)),
                None => Err(TypeError::UnannotatedOverloadCall {
                    name: name.to_string(),
                    parameters: only.parameters.clone(),
                    span: span.clone(),
                }),
            },
            _ => Err(TypeError::AmbiguousOverload {
                name: name.to_string(),
                arg_types: arg_types.to_vec(),
                candidates: candidates(),
                span: span.clone(),
            }),
        }
    }

    /// Register a top-level function definition as a member of its overload set. Each
    /// overloaded member must annotate all its parameter types (exact-type dispatch
    /// can't pick between unannotated members) and its return type — registration runs
    /// before any body is checked, so an omitted return type is recorded as unknown
    /// (`ret: None`) and reported at the first call to the member, or at the definition
    /// if none exists (see `report_unannotated_overload_member`).
    pub(super) fn register_overload_declaration(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), TypeError> {
        // An overload member is bound on its set rather than in `env`, so the reserved-name
        // check `env` runs for every other binding runs here, under the same switch.
        if self.env.enforce_reserved_names
            && let Some(error) = TypeError::reserved_name(&declaration.name, &declaration.span)
        {
            return Err(error);
        }
        let mut parameters = Vec::with_capacity(declaration.parameters.len());
        for (i, p) in declaration.parameters.iter().enumerate() {
            match declaration.parameter_type(i) {
                Some(t) => parameters.push(self.resolve_type(t)),
                // Exact-type dispatch needs every overloaded member's parameters annotated.
                None => {
                    return Err(TypeError::OverloadMissingAnnotation {
                        name: declaration.name.clone(),
                        parameter: p.name.clone(),
                        span: p.span.clone(),
                    });
                }
            }
        }
        let ret = declaration
            .declared_return_type()
            .map(|t| self.resolve_type(t));

        self.finish_overload_registration(
            &declaration.name,
            &declaration.span,
            parameters,
            ret,
            false,
        )
    }

    /// The shared tail of registering one overload member (a top-level function or a type's
    /// operator member): the comparison-must-return-`Bool` rule, exact-duplicate-signature
    /// rejection, unannotated-member tracking, and the `add_overload` itself. Callers differ
    /// only in how they build `parameters`/`ret`.
    ///
    /// A comparison/equality operator (`== != < <= > >=`) is a predicate feeding `?`/`|`
    /// matching and conditionals, so it must return `Bool`; arithmetic operators are
    /// unconstrained. An unannotated return is left to the missing-annotation report. A
    /// duplicate signature would make every call to the name ambiguous.
    pub(super) fn finish_overload_registration(
        &mut self,
        name: &str,
        span: &Span,
        parameters: Vec<Type>,
        ret: Option<Type>,
        is_static: bool,
    ) -> Result<(), TypeError> {
        if is_comparison_operator(name)
            && let Some(ret) = &ret
            && ret != &Type::Bool
        {
            return Err(TypeError::ComparisonOverloadNotBool {
                operator: name.to_string(),
                got: Box::new(ret.clone()),
                span: span.clone(),
            });
        }

        if let Some(set) = self.overloads.get(name)
            && set.iter().any(|o| {
                o.parameters.len() == parameters.len()
                    && o.parameters
                        .iter()
                        .zip(&parameters)
                        .all(|(a, b)| types_match(a, b))
            })
        {
            return Err(TypeError::DuplicateDefinition {
                name: name.to_string(),
                span: span.clone(),
            });
        }

        if ret.is_none() && self.unannotated_overload_member.is_none() {
            self.unannotated_overload_member =
                Some((name.to_string(), parameters.clone(), span.clone()));
        }

        self.add_overload(
            name,
            Overload {
                parameters,
                ret,
                // A user member's aliasing classification is filled in when its body is
                // checked; until then a call to it is assumed to alias every argument.
                result_aliasing: None,
                is_static,
            },
        );
        Ok(())
    }

    /// Record a user overload member's classified result aliasing on the member whose
    /// parameter types these are, once its body has been checked.
    pub(super) fn set_overload_result_aliasing(
        &mut self,
        name: &str,
        parameter_types: &[Type],
        result_aliasing: ResultAliasing,
    ) {
        if let Some(set) = self.overloads.get_mut(name)
            && let Some(member) = set.iter_mut().find(|overload| {
                overload.parameters.len() == parameter_types.len()
                    && overload
                        .parameters
                        .iter()
                        .zip(parameter_types)
                        .all(|(a, b)| types_match(a, b))
            })
        {
            member.result_aliasing = Some(result_aliasing);
        }
    }

    /// Register an operator MEMBER of a record or sum type as a member of that operator's
    /// overload set. An operator lives inside the type it operates on: `it` is the left
    /// operand (its type is `self_type`) and the member's single explicit parameter is the
    /// right operand. So `Color`'s `== = (other :: Color) -> Bool` becomes the `==`
    /// overload `(Color, Color) -> Bool`, and binary-operator dispatch resolves `a == b`
    /// through the same exact-type mechanism as any overload. The render `` ` `` is not an
    /// operator symbol and is handled as a method, not here.
    pub(super) fn register_operator_member(
        &mut self,
        self_type: &Type,
        method: &crate::ast::MethodDeclaration,
    ) -> Result<(), TypeError> {
        // The `%` hash hook is a UNARY member — `it` only, no right operand — that turns a
        // value into a `Num` hash so its type can be a Map/Set key. It has no `.qn` call
        // syntax; the collections invoke it directly. Its overload is `(Self) -> Num`.
        if method.name == "%" && method.parameters.is_empty() {
            let ret = method.return_type.as_ref().map(|t| self.resolve_type(t));
            if ret != Some(Type::Num) {
                let got = match &ret {
                    Some(t) => crate::ast::type_label(t),
                    None => "an unannotated return".to_string(),
                };
                return Err(TypeError::InvalidBuiltinArgument {
                    message: format!("the `%` hash hook must return Num, but returns {got}"),
                    span: method.span.clone(),
                });
            }
            return self.finish_overload_registration(
                &method.name,
                &method.span,
                vec![self_type.clone()],
                ret,
                false,
            );
        }

        // A binary operator member takes exactly one explicit parameter (the right operand).
        if method.parameters.len() != 1 {
            return Err(TypeError::OperatorMemberArity {
                operator: method.name.clone(),
                got: method.parameters.len(),
                span: method.span.clone(),
            });
        }
        let parameter = &method.parameters[0];
        let parameter_type = match &parameter.type_annotation {
            Some(t) => self.resolve_type(t),
            None => {
                return Err(TypeError::OverloadMissingAnnotation {
                    name: method.name.clone(),
                    parameter: parameter.name.clone(),
                    span: parameter.span.clone(),
                });
            }
        };
        let parameters = vec![self_type.clone(), parameter_type];
        let ret = method.return_type.as_ref().map(|t| self.resolve_type(t));

        self.finish_overload_registration(&method.name, &method.span, parameters, ret, false)
    }

    /// After every item is checked, an overload member that never got its return type
    /// annotated is reported at its own definition. A call to one is reported at the call
    /// instead (`resolve_overload`), which runs first — so this only speaks up for a
    /// member nothing calls, where there is no better place to point.
    pub(super) fn report_unannotated_overload_member(&self) -> Result<(), TypeError> {
        match &self.unannotated_overload_member {
            Some((name, parameters, span)) => Err(TypeError::UnannotatedOverloadMember {
                name: name.clone(),
                parameters: parameters.clone(),
                span: span.clone(),
            }),
            None => Ok(()),
        }
    }
}

/// Whether `name` is a comparison/equality operator — these overloads are predicates
/// and are required to return `Bool` (arithmetic operators are unconstrained).
pub(super) fn is_comparison_operator(name: &str) -> bool {
    matches!(name, "==" | "!=" | "<" | "<=" | ">" | ">=")
}
