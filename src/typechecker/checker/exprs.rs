//! Inferring an expression's type — the walk that also records every node's type in the
//! table codegen later reads back.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;

impl TypeChecker {
    /// Type-check an interpolated string: every hole must type-check (any type is
    /// renderable via its `` ` `` operator — built-in default or user override), so the
    /// whole expression is `Text`. Kept separate from `infer_expr_inner` so its loop
    /// locals do not enlarge that hot, deeply-recursive frame (debug builds don't reuse
    /// stack slots, and deep call/pipeline chains recurse ~40 levels through it).
    pub(super) fn check_interpolation(&mut self, parts: &[InterpPart]) -> Result<Type, TypeError> {
        for part in parts {
            if let InterpPart::Hole(e) = part {
                self.infer_expr(e)?;
            }
        }
        Ok(Type::Text)
    }

    /// Infer an expression's type, **recording it in the type oracle** (`type_table`)
    /// keyed by the expression's source span. This is the public inference entry point;
    /// the per-node logic lives in `infer_expr_inner`. The recorded side-table is what
    /// `check_program` returns and codegen consults to recover the precise element /
    /// field / match-result types it would otherwise lose at read sites (see
    /// `TypeOracle` in codegen). Only successfully-typed expressions are recorded.
    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        let ty = self.infer_expr_inner(expr)?;
        self.type_table.insert(expr.span().clone(), ty.clone());
        Ok(ty)
    }

    pub(super) fn infer_expr_inner(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match expr {
            Expr::Number { .. } => Ok(Type::Num),
            Expr::String { .. } => Ok(Type::Text),
            // Delegated to a separate method so its locals stay OUT of this hot,
            // deeply-recursive frame (debug builds don't reuse stack slots).
            Expr::Interpolation { parts, .. } => self.check_interpolation(parts),
            Expr::Bool { .. } => Ok(Type::Bool),
            Expr::Unit { .. } => Ok(Type::Unit),

            Expr::Ident { name, span } => {
                self.env
                    .get_type(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: span.clone(),
                    })
            }

            Expr::BinOp {
                left,
                op,
                right,
                span,
            } => self.check_binop(left, *op, right, span),

            Expr::UnaryOp { op, expr, span } => self.check_unary_op(*op, expr, span),

            Expr::Call { func, args, span } => self.check_call(func, args, span),

            Expr::Lambda {
                params,
                return_type,
                body,
                ..
            } => self.check_lambda(params, return_type.as_ref(), body),

            Expr::Pipeline { left, right, span } => {
                // `left |> right` injects `left` as the first argument of the
                // right-hand call: `x |> f` => `f(x)`, `x |> f(a)` => `f(x, a)`.
                // Desugar and type-check the resulting call (shared with codegen).
                let call = Expr::desugar_pipeline(left, right, span);
                self.infer_expr(&call)
            }

            Expr::Block { stmts, span: _ } => {
                if stmts.is_empty() {
                    return Ok(Type::Num); // Default to Num for empty blocks
                }

                // Process statements in order, last one is the result
                let mut result_type = Type::Num;

                for stmt in stmts.iter() {
                    match stmt {
                        crate::ast::Statement::Item(item) => {
                            self.check_item(item, Nesting::Nested)?;
                        }
                        crate::ast::Statement::Expr(expr) => {
                            result_type = self.infer_expr(expr)?;
                        }
                    }
                }

                Ok(result_type)
            }

            Expr::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_type = self.infer_expr(cond)?;
                self.check_type_compatibility(&Type::Bool, &cond_type, span)?;

                let then_type = self.infer_expr(then)?;
                let else_type = self.infer_expr(else_)?;

                self.check_type_compatibility(&then_type, &else_type, span)?;
                // Merge the branch types so a `Result` gets the concrete payload from
                // whichever branch specialized each variant (`Ok("x") : NotOk("e")` =>
                // `Result[Ok(Text), NotOk(Text)]`), letting both match arms bind usably.
                Ok(Self::merge_types(then_type, &else_type))
            }

            Expr::Match { expr, arms, span } => self.check_match(expr, arms, span),

            Expr::FieldAccess { expr, field, span } => {
                let expr_type = self.infer_expr(expr)?;

                match expr_type {
                    Type::Record(fields) => {
                        for (f, t) in fields {
                            if f == *field {
                                return Ok(t);
                            }
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    Type::Named {
                        name: _,
                        fields,
                        methods: _,
                    } => {
                        // Handle field access on named types
                        for (f, t) in fields.iter() {
                            if f == field {
                                return Ok(t.clone());
                            }
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    Type::Array(_elem_type) => {
                        // Arrays have a built-in .size field
                        if field == "size" {
                            return Ok(Type::Num);
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    Type::Text => {
                        // Text has `.size` (byte length) and `.length` (grapheme count).
                        if field == "size" || field == "length" {
                            return Ok(Type::Num);
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    _ => Err(TypeError::TypeMismatch {
                        expected: Box::new(Type::Record(vec![])),
                        got: Box::new(expr_type),
                        span: span.clone(),
                    }),
                }
            }

            Expr::FieldAssign {
                target,
                value,
                span,
            } => {
                // `obj.field := v`: the field's declared type must accept `v`, and the
                // root binding of the path must be mutable (`:=`-bound) — writing a
                // field of an immutable (`=`) instance is a compile error.
                //
                // Infer the target first so an undefined root variable / unknown field
                // surfaces its own diagnostic, rather than being misreported as an
                // immutable write (an unknown name reads as "not mutable").
                let field_type = self.infer_expr(target)?;

                if let Some(name) = self.immutable_mutation_root(target) {
                    return Err(TypeError::ImmutableFieldWrite {
                        name,
                        span: span.clone(),
                    });
                }

                let value_type = self.infer_expr(value)?;
                self.check_type_compatibility(&field_type, &value_type, span)?;

                // A field write is an effect; its value is the unit type `$`.
                Ok(Type::Unit)
            }

            Expr::Index { expr, index, span } => {
                let expr_type = self.infer_expr(expr)?;
                let index_type = self.infer_expr(index)?;

                // Index must be Num
                if index_type != Type::Num {
                    return Err(TypeError::TypeMismatch {
                        expected: Box::new(Type::Num),
                        got: Box::new(index_type),
                        span: span.clone(),
                    });
                }

                // Expression must be an array
                match expr_type {
                    Type::Array(elem_type) => Ok(*elem_type),
                    _ => Err(TypeError::TypeMismatch {
                        expected: Box::new(Type::Array(Box::new(Type::Num))),
                        got: Box::new(expr_type),
                        span: span.clone(),
                    }),
                }
            }

            Expr::Array { elements, span } => {
                if elements.is_empty() {
                    // Empty array - infer as Array(Num) for now
                    return Ok(Type::Array(Box::new(Type::Num)));
                }

                // The element type each element contributes: a plain element contributes
                // its own type; a `<-source` spread contributes the ELEMENT type of the
                // source array (which must itself be an array). All contributions must be
                // mutually compatible, and the result is `[]elem`.
                let mut elem_type: Option<Type> = None;
                for elem in elements {
                    let contributed = if let Expr::Spread { expr: src, .. } = elem {
                        // Record the spread node's type (= the source array's type).
                        let src_type = self.infer_expr(elem)?;
                        match src_type {
                            Type::Array(inner) => (*inner).clone(),
                            other => {
                                return Err(TypeError::TypeMismatch {
                                    expected: Box::new(Type::Array(Box::new(Type::Num))),
                                    got: Box::new(other),
                                    span: src.span().clone(),
                                });
                            }
                        }
                    } else {
                        self.infer_expr(elem)?
                    };
                    match &elem_type {
                        None => elem_type = Some(contributed),
                        Some(first) => self.check_type_compatibility(first, &contributed, span)?,
                    }
                }

                Ok(Type::Array(Box::new(elem_type.unwrap_or(Type::Num))))
            }

            Expr::Record { fields, .. } => self.infer_record(fields),

            Expr::Spread { expr, .. } => {
                // A spread's own type is the type of its source; the surrounding array /
                // record literal interprets it (element splice / field merge). A bare
                // spread outside a literal never reaches codegen (the parser only produces
                // one inside `[ ]` / `{ }`), so recording the source type here suffices.
                self.infer_expr(expr)
            }

            Expr::Constructor {
                type_name,
                fields,
                span,
            } => {
                // Look up the type definition
                if let Some(symbol) = self.env.lookup(type_name) {
                    match &symbol.type_ {
                        Type::Named {
                            name,
                            fields: type_fields,
                            methods,
                        } => {
                            // Clone the type info to avoid borrow issues
                            let name = name.clone();
                            let type_fields = type_fields.clone();
                            let methods = methods.clone();

                            // Type-check each field
                            let mut provided_fields = std::collections::HashSet::new();

                            for (field_name, field_expr) in fields {
                                // A `<-source` entry fills every declared field at once.
                                // The source must already BE this type, or be an
                                // anonymous record of exactly its shape — a different
                                // named type is not interchangeable with this one, and a
                                // record cannot stand in for a type that has methods it
                                // does not carry.
                                if let Expr::Spread { expr: src, .. } = field_expr {
                                    let src_type = self.infer_expr(src)?;
                                    let fills = match &src_type {
                                        Type::Named { name: src_name, .. } => src_name == &name,
                                        Type::Record(src_fields) => {
                                            methods.is_empty()
                                                && src_fields.len() == type_fields.len()
                                                && type_fields.iter().all(|(f, ty)| {
                                                    src_fields.iter().any(|(sf, sty)| {
                                                        sf == f && types_match(ty, sty)
                                                    })
                                                })
                                        }
                                        _ => false,
                                    };
                                    if !fills {
                                        return Err(TypeError::TypeMismatch {
                                            expected: Box::new(Type::Named {
                                                name: name.clone(),
                                                fields: type_fields.clone(),
                                                methods: methods.clone(),
                                            }),
                                            got: Box::new(src_type),
                                            span: span.clone(),
                                        });
                                    }
                                    for (f, _) in type_fields.iter() {
                                        provided_fields.insert(f.clone());
                                    }
                                    continue;
                                }

                                provided_fields.insert(field_name.clone());

                                // Find the expected type for this field
                                let expected_type = type_fields
                                    .iter()
                                    .find(|(f, _)| f == field_name)
                                    .map(|(_, t)| t.clone())
                                    .ok_or_else(|| TypeError::UndefinedVariable {
                                        name: format!("field {} in type {}", field_name, type_name),
                                        span: span.clone(),
                                    })?;

                                // Type-check the field value
                                let actual_type = self.infer_expr(field_expr)?;
                                self.check_type_compatibility(&expected_type, &actual_type, span)?;
                            }

                            // Check all fields are provided
                            for (field_name, _) in type_fields.iter() {
                                if !provided_fields.contains(field_name) {
                                    return Err(TypeError::UndefinedVariable {
                                        name: format!(
                                            "Missing field {} in constructor for {}",
                                            field_name, type_name
                                        ),
                                        span: span.clone(),
                                    });
                                }
                            }

                            // Return the Named type
                            Ok(Type::Named {
                                name,
                                fields: type_fields,
                                methods,
                            })
                        }
                        _ => Err(TypeError::TypeMismatch {
                            expected: Box::new(Type::named_ref(type_name.clone())),
                            got: Box::new(symbol.type_.clone()),
                            span: span.clone(),
                        }),
                    }
                } else {
                    Err(TypeError::UndefinedVariable {
                        name: type_name.clone(),
                        span: span.clone(),
                    })
                }
            }

            Expr::Range { start, end, span } => {
                // `lo <- hi` materializes an inclusive `[]Num`; both ends must be Num.
                let start_type = self.infer_expr(start)?;
                self.check_type_compatibility(&Type::Num, &start_type, span)?;
                let end_type = self.infer_expr(end)?;
                self.check_type_compatibility(&Type::Num, &end_type, span)?;
                Ok(Type::Array(Box::new(Type::Num)))
            }
        }
    }

    /// Infer the type of a record literal, expanding any `<-source` spreads (functional
    /// update). Fields merge left-to-right: a spread splices all of its source record's
    /// fields; a later entry (from a spread or an explicit `name = v`) OVERRIDES an
    /// earlier one of the same name, and a new name is appended. If the FIRST named-record
    /// spread source's declared field set is reproduced exactly by the merged result (same
    /// names, each type compatible, nothing added), the result KEEPS that named type — and
    /// therefore its methods; otherwise it is an anonymous record.
    pub(super) fn infer_record(&mut self, fields: &[(String, Expr)]) -> Result<Type, TypeError> {
        let mut merged: Vec<(String, Type)> = Vec::new();
        // The named type of the FIRST named-record spread source, if any (holds its
        // declared fields + methods) — the candidate the result may keep.
        let mut named_identity: Option<Type> = None;

        for (name, value) in fields {
            if let Expr::Spread { expr: src, .. } = value {
                let src_type = self.infer_expr(value)?;
                let src_fields = match &src_type {
                    Type::Record(fs) => fs.clone(),
                    Type::Named { fields: fs, .. } => {
                        if named_identity.is_none() {
                            named_identity = Some(src_type.clone());
                        }
                        fs.to_vec()
                    }
                    other => {
                        return Err(TypeError::TypeMismatch {
                            expected: Box::new(Type::Record(vec![])),
                            got: Box::new(other.clone()),
                            span: src.span().clone(),
                        });
                    }
                };
                for (fname, fty) in src_fields {
                    match merged.iter_mut().find(|(n, _)| *n == fname) {
                        Some(slot) => slot.1 = fty,
                        None => merged.push((fname, fty)),
                    }
                }
            } else {
                let value_type = self.infer_expr(value)?;
                match merged.iter_mut().find(|(n, _)| *n == *name) {
                    Some(slot) => slot.1 = value_type,
                    None => merged.push((name.clone(), value_type)),
                }
            }
        }

        // Preserve the named type (and its methods) only if the merged field set is
        // exactly the named type's declared fields, each with a compatible type.
        if let Some(Type::Named {
            fields: decl_fields,
            ..
        }) = &named_identity
        {
            let reproduces_named = merged.len() == decl_fields.len()
                && decl_fields.iter().all(|(dn, dt)| {
                    merged
                        .iter()
                        .find(|(mn, _)| mn == dn)
                        .is_some_and(|(_, mt)| Self::types_compatible(dt, mt))
                });
            if reproduces_named {
                return Ok(named_identity.unwrap());
            }
        }

        Ok(Type::Record(merged))
    }

    pub(super) fn check_binop(
        &mut self,
        left: &Expr,
        op: BinOp,
        right: &Expr,
        span: &Span,
    ) -> Result<Type, TypeError> {
        let left_type = self.infer_expr(left)?;
        let right_type = self.infer_expr(right)?;

        // `+` on arrays always builds a NEW array (neither operand is mutated), in three
        // exact-type-dispatched forms — polymorphic over the element type `T`, so they
        // can't be fixed builtin overload members and are resolved here, before overload
        // dispatch:
        //   concat:  `[]T + []T -> []T`   (both sides arrays of the SAME element type)
        //   append:  `[]T + T   -> []T`   (right matches the left array's element type)
        //   prepend: `T   + []T -> []T`   (left matches the right array's element type)
        // The forms are mutually exclusive (`[]T` can never equal its own element `T`),
        // so there is no ambiguity — including the nested case `[][]Num + []Num`, where
        // the right (`[]Num`) equals the element type and so binds as APPEND (a single
        // element), yielding `[][]Num`. Anything else involving an array operand (e.g.
        // mismatched element types) is a clear type error.
        if op == BinOp::Add
            && (matches!(left_type, Type::Array(_)) || matches!(right_type, Type::Array(_)))
        {
            match (&left_type, &right_type) {
                // concat: `[]T + []T` — same element type on both sides.
                (Type::Array(l_elem), Type::Array(r_elem)) if types_match(l_elem, r_elem) => {
                    return Ok(left_type.clone());
                }
                // append: `[]T + T` — right is a single element of the left array's type.
                (Type::Array(l_elem), r) if types_match(l_elem, r) => {
                    return Ok(left_type.clone());
                }
                // prepend: `T + []T` — left is a single element of the right array's type.
                (l, Type::Array(r_elem)) if types_match(r_elem, l) => {
                    return Ok(right_type.clone());
                }
                _ => {}
            }
            return Err(TypeError::TypeMismatch {
                expected: Box::new(left_type),
                got: Box::new(right_type),
                span: span.clone(),
            });
        }

        // An operator is just a named overload set. Resolve it by exact operand types
        // against the operator's overload set, which holds the built-in members
        // (Num/Text `+`, the comparisons, …) PLUS any user-defined operator overloads
        // — so built-ins and user operators dispatch through the same mechanism.
        self.resolve_overload(op.symbol(), &[left_type, right_type], span)
    }

    pub(super) fn check_unary_op(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
        span: &Span,
    ) -> Result<Type, TypeError> {
        let expr_type = self.infer_expr(expr)?;

        match op {
            UnaryOp::Neg => {
                self.check_type_compatibility(&Type::Num, &expr_type, span)?;
                Ok(Type::Num)
            }
            UnaryOp::Not => {
                self.check_type_compatibility(&Type::Bool, &expr_type, span)?;
                Ok(Type::Bool)
            }
        }
    }
}
