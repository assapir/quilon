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
    /// `print`/`eprint` get a member per printable built-in (`Num`/`Text`/`Bool`).
    pub(super) fn add_builtin_overloads(&mut self) {
        let arith = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Mod];
        for op in arith {
            // Num op Num -> Num.
            self.add_overload(
                op.symbol(),
                Overload {
                    params: vec![Type::Num, Type::Num],
                    ret: Some(Type::Num),
                },
            );
        }
        // `+` also concatenates Text.
        self.add_overload(
            BinOp::Add.symbol(),
            Overload {
                params: vec![Type::Text, Type::Text],
                ret: Some(Type::Text),
            },
        );

        // Comparisons. Equality (`==`/`!=`) over every built-in scalar; ordering
        // (`<`/`<=`/`>`/`>=`) over Num and Text (Text is lexicographic — the
        // concrete deliverable). All yield Bool.
        let eq_ops = [BinOp::Eq, BinOp::Ne];
        let ord_ops = [BinOp::Lt, BinOp::Le, BinOp::Gt, BinOp::Ge];
        for op in eq_ops {
            for ty in [Type::Num, Type::Text, Type::Bool] {
                self.add_overload(
                    op.symbol(),
                    Overload {
                        params: vec![ty.clone(), ty],
                        ret: Some(Type::Bool),
                    },
                );
            }
        }
        for op in ord_ops {
            for ty in [Type::Num, Type::Text] {
                self.add_overload(
                    op.symbol(),
                    Overload {
                        params: vec![ty.clone(), ty],
                        ret: Some(Type::Bool),
                    },
                );
            }
        }

        // Logical `&&`/`||`: Bool op Bool -> Bool.
        for op in [BinOp::And, BinOp::Or] {
            self.add_overload(
                op.symbol(),
                Overload {
                    params: vec![Type::Bool, Type::Bool],
                    ret: Some(Type::Bool),
                },
            );
        }

        // `print`/`eprint`: one member per printable built-in; all return `$` (Unit).
        for name in ["print", "eprint"] {
            for ty in [Type::Num, Type::Text, Type::Bool] {
                self.add_overload(
                    name,
                    Overload {
                        params: vec![ty],
                        ret: Some(Type::Unit),
                    },
                );
            }
        }

        // `__exit(code :: Num) -> $` — the internal process-exit primitive `core.test`
        // builds on (its `assert` calls `__exit(101)` to fail). Codegen lowers it to the
        // `__exit` runtime intrinsic. Registered as a builtin so a corelib `.ql` (and,
        // by the same token, any program) can call it by name; it is deliberately
        // `__`-prefixed to mark it internal — there is no user-facing `exit`.
        self.add_overload(
            "__exit",
            Overload {
                params: vec![Type::Num],
                ret: Some(Type::Unit),
            },
        );
    }

    /// Add one member to the overload set `name`.
    pub(super) fn add_overload(&mut self, name: &str, overload: Overload) {
        self.overloads
            .entry(name.to_string())
            .or_default()
            .push(overload);
    }

    /// Whether overload set `name` has a member whose parameters EXACTLY match `arg_types`
    /// (no coercion) — a non-erroring probe used to decide whether `print`/`eprint` should
    /// take the generic render path or dispatch to a concrete overload. A member whose
    /// LAST parameter is the built-in `Site` also matches one argument short of it: that
    /// argument is the caller's location, which the compiler fills in.
    pub(super) fn has_exact_overload(&self, name: &str, arg_types: &[Type]) -> bool {
        self.overloads.get(name).is_some_and(|set| {
            set.iter()
                .any(|o| crate::ast::params_accept(&o.params, arg_types, types_match))
        })
    }

    /// Resolve a call to overload set `name` by EXACT argument-type match (no implicit
    /// coercion). Returns the matched overload's return type. Errors on no match or
    /// (with exact matching, a duplicate-signature) ambiguity, listing the candidates.
    pub(super) fn resolve_overload(
        &self,
        name: &str,
        arg_types: &[Type],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let set = self.overloads.get(name);
        let matches: Vec<&Overload> = set
            .map(|s| {
                s.iter()
                    .filter(|o| crate::ast::params_accept(&o.params, arg_types, types_match))
                    .collect()
            })
            .unwrap_or_default();

        // Candidate signatures are only needed to render an error, so build them lazily.
        let candidates = || -> Vec<Vec<Type>> {
            set.map(|s| s.iter().map(|o| o.params.clone()).collect())
                .unwrap_or_default()
        };

        match matches.as_slice() {
            [] => Err(TypeError::NoMatchingOverload {
                name: name.to_string(),
                arg_types: arg_types.to_vec(),
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
                    params: only.params.clone(),
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
    pub(super) fn register_overload_decl(&mut self, decl: &FunctionDecl) -> Result<(), TypeError> {
        let mut params = Vec::with_capacity(decl.params.len());
        for p in &decl.params {
            match &p.type_annotation {
                Some(t) => params.push(self.resolve_type(t)),
                // Exact-type dispatch needs every overloaded member's params annotated.
                None => {
                    return Err(TypeError::OverloadMissingAnnotation {
                        name: decl.name.clone(),
                        param: p.name.clone(),
                        span: p.span.clone(),
                    });
                }
            }
        }
        let ret = decl.return_type.as_ref().map(|t| self.resolve_type(t));

        // A comparison/equality operator overload (`== != < <= > >=`) must return `Bool`:
        // these are predicates that feed `?`/`|` matching and conditionals. (Arithmetic
        // operators are unconstrained — e.g. `Vec * Num -> Vec` is fine.) An unannotated
        // one is left to the missing-return-annotation report, which says what to do.
        if is_comparison_operator(&decl.name)
            && let Some(ret) = &ret
            && ret != &Type::Bool
        {
            return Err(TypeError::ComparisonOverloadNotBool {
                operator: decl.name.clone(),
                got: Box::new(ret.clone()),
                span: decl.span.clone(),
            });
        }

        // Reject an exact-duplicate signature (same parameter types) up front — it
        // would make every call to it ambiguous.
        if let Some(set) = self.overloads.get(&decl.name)
            && set.iter().any(|o| {
                o.params.len() == params.len()
                    && o.params.iter().zip(&params).all(|(a, b)| types_match(a, b))
            })
        {
            return Err(TypeError::DuplicateDefinition {
                name: decl.name.clone(),
                span: decl.span.clone(),
            });
        }

        if ret.is_none() && self.unannotated_overload_member.is_none() {
            self.unannotated_overload_member =
                Some((decl.name.clone(), params.clone(), decl.span.clone()));
        }

        self.add_overload(&decl.name, Overload { params, ret });
        Ok(())
    }

    /// After every item is checked, an overload member that never got its return type
    /// annotated is reported at its own definition. A call to one is reported at the call
    /// instead (`resolve_overload`), which runs first — so this only speaks up for a
    /// member nothing calls, where there is no better place to point.
    pub(super) fn report_unannotated_overload_member(&self) -> Result<(), TypeError> {
        match &self.unannotated_overload_member {
            Some((name, params, span)) => Err(TypeError::UnannotatedOverloadMember {
                name: name.clone(),
                params: params.clone(),
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
