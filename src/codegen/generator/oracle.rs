//! The type queries codegen runs before emitting anything: what the checker inferred for
//! an expression (the type oracle), and how a Quilon type is represented in LLVM.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// The `{ ptr data, i64 len }` struct shared by arrays and `Text`. For `Text`,
    /// `data` is a NUL-terminated UTF-8 buffer and `len` is its byte length.
    pub(super) fn ptr_len_struct_type(&self) -> inkwell::types::StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            false,
        )
    }

    /// The single value of the Unit type (`$`), lowered as a zero `i8`. Its bits are
    /// never observed; the entry-point wrapper coerces a non-Num body to exit code 0.
    pub(super) fn unit_value(&self) -> inkwell::values::IntValue<'ctx> {
        self.context.i8_type().const_int(0, false)
    }

    /// Whether `expr`'s value has type Unit (`$`). Codegen lacks the checker's full
    /// inference, so for an *unannotated* function we look at the body's tail to pick
    /// the LLVM return type: a Unit tail must be `i8`, not the `Num`/f64 default. The
    /// only Unit-producing expressions are the `$` literal and `print`/`eprint` calls
    /// (which return `$`); a block/ternary is Unit when its tail is. Other unannotated
    /// non-Num bodies (Text, Bool, ...) keep the pre-existing `Num`-default behavior.
    pub(super) fn expr_is_unit(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Unit { .. } => true,
            // An in-place field write `obj.field := v` is an effect; it yields `$`.
            Expr::FieldAssign { .. } => true,
            Expr::Call { func, .. } => {
                matches!(func.as_ref(), Expr::Ident { name, .. } if name == "print" || name == "eprint")
            }
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.expr_is_unit(tail),
                _ => false,
            },
            Expr::If { then, else_, .. } => self.expr_is_unit(then) && self.expr_is_unit(else_),
            _ => false,
        }
    }

    /// The **value representation** of a Quilon type — the LLVM type that a value of
    /// `ty` is materialized as by `generate_expr` and stored inline inside a composite.
    /// Read sites that GEP/load an element/field/match-result must size it with THIS
    /// function so the type matches how the value was stored at construction. It differs
    /// from [`type_to_llvm`] in three places:
    ///   - `Array` — an array *value* is the `{ ptr, i64 }` struct `generate_array`
    ///     produces and stores inline (so a nested array `[][]T` keeps that struct as its
    ///     element), whereas `type_to_llvm` lowers `[]T` to a bare opaque pointer.
    ///   - `Record` / `Named` — a record *value* is a POINTER to its struct (the record
    ///     ABI: `generate_record` returns the alloca), not the struct by value.
    ///   - `Generic` — a payload type variable that survived to a read site (e.g. a match
    ///     whose result type was taken from a never-constructed variant's generic arm)
    ///     has no concrete LLVM type; it falls back to the canonical numeric payload
    ///     representation `f64`, matching how generic/unknown payloads are materialized
    ///     elsewhere (`payload_slot_type`). This keeps such a program compiling (it did
    ///     before the oracle existed) rather than erroring in `type_to_llvm`.
    pub(super) fn value_repr_type(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Array(_) => Ok(self.ptr_len_struct_type().into()),
            Type::Record(_) | Type::Named { .. } => {
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Generic { .. } => Ok(self.context.f64_type().into()),
            _ => self.type_to_llvm(ty),
        }
    }

    /// The LLVM type a value of `ty` takes when it CROSSES a function boundary — a param
    /// or a return, for top-level functions, methods, and closures alike. An array must
    /// use its VALUE representation (the `{ ptr, i64 }` struct, so callers can `.size` /
    /// index / concatenate the result), matching how array values flow everywhere else;
    /// everything else keeps its `type_to_llvm` lowering. This is deliberately NOT the
    /// whole of [`value_repr_type`]: a `Record`/`Named` argument keeps its by-pointer ABI
    /// and a `Generic` keeps `type_to_llvm`, so only the array case diverges here. Every
    /// signature site funnels through this one method so the boundary rule lives in a
    /// single place.
    pub(super) fn boundary_type(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Array(_) => self.value_repr_type(ty),
            _ => self.type_to_llvm(ty),
        }
    }

    /// The value-representation LLVM type to use when GEPing/loading the result of `expr`
    /// (an `arr[i]`, `rec.field`, or `match`), taken from the type oracle. This is the
    /// single read-site policy: ask the oracle for `expr`'s inferred type and lower it
    /// via [`value_repr_type`]; if the oracle has no entry (e.g. the IR-only codegen
    /// tests that skip the type-check pass), fall back to the historical `f64`.
    pub(super) fn oracle_value_type(&self, expr: &Expr) -> Result<BasicTypeEnum<'ctx>, String> {
        match self.oracle.expr_type(expr) {
            Some(t) => self.value_repr_type(t),
            None => Ok(self.context.f64_type().into()),
        }
    }

    /// Best-effort Quilon type of `expr`, sufficient to mangle overloaded call sites.
    /// Codegen lacks the type checker's full inference, so this covers exactly the
    /// shapes that can be an overloaded argument: literals, locals/params (tracked in
    /// `var_types`), constructor results, field access on a known record, and the
    /// result types of the supported operators. Falls back to `Num` (the historical
    /// default) when it can't tell — overloaded dispatch then simply won't match and a
    /// clear "function not found" surfaces, never a silent miscompile.
    pub(super) fn infer_type(&self, expr: &Expr) -> Type {
        // Prefer the type checker's authoritative type for this exact node (the oracle) —
        // the same source codegen's read sites use. It knows shapes the structural fallback
        // below can't (an `arr[i]` element, a `.split(…)`/`.replace(…)` result, a field
        // read), so an overloaded call taking one of those (e.g. `assertEq(parts[0], …)`)
        // mangles to the right member. Falls back to structural inference only when the
        // oracle has no entry — the IR-only codegen tests that skip the type-check pass.
        if let Some(ty) = self.oracle.expr_type(expr) {
            return ty.clone();
        }
        match expr {
            Expr::Number { .. } => Type::Num,
            Expr::String { .. } => Type::Text,
            Expr::Bool { .. } => Type::Bool,
            Expr::Unit { .. } => Type::Unit,
            Expr::Ident { name, .. } => {
                // A bare nullary sum-type constructor (not a bound variable) is a value
                // of its sum type.
                if let Some((_, type_name)) = self.sum_variants.get(name)
                    && !self.var_types.contains_key(name)
                {
                    return self.sum_or_named(type_name);
                }
                self.var_types.get(name).cloned().unwrap_or(Type::Num)
            }
            Expr::Constructor { type_name, .. } => self.sum_or_named(type_name),
            Expr::Call { func, args, .. } => {
                if let Expr::Ident { name, .. } = func.as_ref() {
                    // A constructor call yields its sum type.
                    if let Some((_, type_name)) = self.sum_variants.get(name) {
                        return self.sum_or_named(type_name);
                    }
                    // An overloaded function call yields its resolved member's return.
                    let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                    if let Some((_, ret)) = self.matching_overload(name, &arg_types) {
                        return ret.clone();
                    }
                    // A non-overloaded top-level function yields its declared return
                    // type — so a call result that feeds an overloaded call/operator
                    // mangles to the right member (codegen agrees with the checker).
                    if let Some(ret) = self.fn_return_types.get(name) {
                        return self.resolve_named(ret);
                    }
                }
                // Unknown callee (no declared return, e.g. an unannotated function):
                // default to Num, the historical inference default.
                Type::Num
            }
            Expr::BinOp {
                left, op, right, ..
            } => {
                // A user operator overload yields its resolved member's return type.
                let sym = op.symbol();
                if self.overloads.contains_key(sym) {
                    let arg_types = [self.infer_type(left), self.infer_type(right)];
                    if let Some((_, ret)) = self.matching_overload(sym, &arg_types) {
                        return ret.clone();
                    }
                }
                // Built-ins: comparisons/logicals yield Bool; `+` on Text yields Text;
                // arithmetic yields Num. Matches the type checker's operator results
                // closely enough to mangle a nested overloaded argument.
                match op {
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => Type::Bool,
                    BinOp::Add
                        if self.infer_type(left) == Type::Text
                            || self.infer_type(right) == Type::Text =>
                    {
                        Type::Text
                    }
                    _ => Type::Num,
                }
            }
            Expr::If { then, .. } => self.infer_type(then),
            // A `?`/`|` match's result type is whatever its arms yield. Codegen can't
            // easily unify the arms, so take the checker's recorded type from the oracle
            // (as record/spread do); this lets a local bound to a match — e.g.
            // `ok = r ? | Ok(_) => true | NotOk(_) => false` — mangle correctly when it
            // later feeds an overloaded call such as `assert(ok)`.
            Expr::Match { .. } => self.oracle.expr_type(expr).cloned().unwrap_or(Type::Num),
            // Unary `!` is logical-not (Bool); unary `-` is numeric negation (Num). So a
            // local bound to `!ok` mangles as Bool when it feeds an overloaded call.
            Expr::UnaryOp { op, .. } => match op {
                crate::ast::UnaryOp::Not => Type::Bool,
                crate::ast::UnaryOp::Neg => Type::Num,
            },
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.infer_type(tail),
                _ => Type::Num,
            },
            Expr::FieldAccess { field, .. } if field == "size" || field == "length" => Type::Num,
            // A record literal / spread — including a functional-update — takes its type
            // from the oracle (which resolves the named-vs-anonymous result of a `<-`
            // spread), so a binding to it mangles / tracks correctly.
            Expr::Record { .. } | Expr::Spread { .. } => {
                self.oracle.expr_type(expr).cloned().unwrap_or(Type::Num)
            }
            _ => Type::Num,
        }
    }

    /// Normalize a declared type annotation for `infer_type`: a bare `Named { name }`
    /// (the parser's form for a `:: SomeType` reference) becomes the canonical sum/named
    /// tag so it mangles identically to an inferred value of that type. Built-ins pass
    /// through unchanged.
    pub(super) fn resolve_named(&self, ty: &Type) -> Type {
        match ty {
            Type::Named { name, .. } | Type::Sum { name, .. } => self.sum_or_named(name),
            other => other.clone(),
        }
    }

    /// The `Type` for a registered type name: a sum type if known, else a `Named`.
    pub(super) fn sum_or_named(&self, name: &str) -> Type {
        if self.sum_layouts.contains_key(name) || name == "Result" {
            Type::Sum {
                name: name.to_string(),
                variants: vec![],
            }
        } else {
            Type::named_ref(name)
        }
    }

    /// If `name` is an overload set, pick the member matching `arg_types` exactly and
    /// return its mangled LLVM symbol. `None` if `name` isn't overloaded or nothing
    /// matches (the caller then falls back to its non-overloaded path).
    pub(super) fn resolve_overload_symbol(&self, name: &str, arg_types: &[Type]) -> Option<String> {
        let (params, _) = self.matching_overload(name, arg_types)?;
        Some(mangle_overload(name, params))
    }

    /// The overload member of `name` whose parameter types match `arg_types` exactly
    /// (by type tag), if any. Shared by symbol resolution and return-type inference.
    pub(super) fn matching_overload(
        &self,
        name: &str,
        arg_types: &[Type],
    ) -> Option<&(Vec<Type>, Type)> {
        self.overloads.get(name)?.iter().find(|(params, _)| {
            params.len() == arg_types.len()
                && params
                    .iter()
                    .zip(arg_types)
                    .all(|(p, a)| type_mangle(p) == type_mangle(a))
        })
    }

    pub(super) fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            // Unit (`$`) is a zero `i8` — a concrete one-inhabitant placeholder.
            Type::Unit => Ok(self.context.i8_type().into()),
            // Text is { ptr data, i64 byte_len } (same shape as an array).
            Type::Text => Ok(self.ptr_len_struct_type().into()),
            Type::Array(elem_type) => {
                // Validate the element type, but LLVM uses opaque pointers so the
                // pointee type is not encoded in the pointer itself.
                let _elem = self.type_to_llvm(elem_type)?;
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Record(fields) => {
                let field_types: Vec<BasicTypeEnum> = fields
                    .iter()
                    .map(|(_name, ty)| self.type_to_llvm(ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.context.struct_type(&field_types, false).into())
            }
            Type::Sum { name, variants } => Ok(self.sum_value_struct_type(name, variants)?.into()),
            // A `Named` reference with no fields is a parsed type annotation (e.g. a
            // function param `s :: Shape`). If it names a registered sum type, lower it
            // to that type's tagged-union struct.
            Type::Named { name, fields, .. }
                if fields.is_empty() && self.sum_layouts.contains_key(name) =>
            {
                Ok(self.sum_struct_type(name).into())
            }
            // Any other named RECORD type (a `:: SomeRecord` parameter/return, e.g. on a
            // user operator overload) is passed by pointer — record instances are
            // represented as a pointer to their struct alloca (see `generate_record`).
            Type::Named { .. } => Ok(self.context.ptr_type(AddressSpace::default()).into()),
            _ => Err(format!("Unsupported type: {:?}", ty)),
        }
    }
}

impl TypeOracle {
    pub(super) fn new(table: crate::typechecker::TypeTable) -> Self {
        Self { table }
    }

    /// The inferred type of `expr`, by its span. `None` if the checker didn't record it.
    pub(super) fn expr_type(&self, expr: &Expr) -> Option<&Type> {
        self.table.get(expr.span())
    }
}

/// A zero/`undef`-free constant of any basic LLVM type, used to fill a payload slot that
/// carries no information (a `$` Unit payload stored into a sized slot).
pub(super) fn zeroed(ty: BasicTypeEnum<'_>) -> BasicValueEnum<'_> {
    match ty {
        BasicTypeEnum::IntType(t) => t.const_zero().into(),
        BasicTypeEnum::FloatType(t) => t.const_zero().into(),
        BasicTypeEnum::PointerType(t) => t.const_zero().into(),
        BasicTypeEnum::StructType(t) => t.const_zero().into(),
        BasicTypeEnum::ArrayType(t) => t.const_zero().into(),
        BasicTypeEnum::VectorType(t) => t.const_zero().into(),
        BasicTypeEnum::ScalableVectorType(t) => t.const_zero().into(),
    }
}
