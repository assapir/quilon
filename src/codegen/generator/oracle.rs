//! The type queries codegen runs before emitting anything: what the checker inferred for
//! an expression (the type oracle), and how a Quilon type is represented in LLVM.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// The `{ ptr data, i64 len }` struct shared by arrays and `Text`. For `Text`, `data`
    /// points at UTF-8 bytes and `len` is how many of them there are — the pair is the whole
    /// value, so nothing reads past `len`.
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

    /// Whether `expression`'s value has type Unit (`$`). Codegen lacks the checker's full
    /// inference, so for an *unannotated* function we look at the body's tail to pick
    /// the LLVM return type: a Unit tail must be `i8`, not the `Num`/f64 default. The
    /// only Unit-producing expressions are the `$` literal and `print`/`eprint` calls
    /// (which return `$`); a block/ternary is Unit when its tail is. Other unannotated
    /// non-Num bodies (Text, Bool, ...) keep the pre-existing `Num`-default behavior.
    pub(super) fn expression_is_unit(&self, expression: &Expression) -> bool {
        match expression {
            Expression::Unit { .. } => true,
            // An in-place field write `obj.field := v` is an effect; it yields `$`.
            Expression::FieldAssign { .. } => true,
            Expression::Call { function, .. } => {
                matches!(function.as_ref(), Expression::Identifier { name, .. } if name == "print" || name == "eprint")
            }
            Expression::Block { statements, .. } => match statements.last() {
                Some(crate::ast::Statement::Expression(tail)) => self.expression_is_unit(tail),
                _ => false,
            },
            Expression::If { then, else_, .. } => {
                self.expression_is_unit(then) && self.expression_is_unit(else_)
            }
            _ => false,
        }
    }

    /// The **value representation** of a Quilon type — the LLVM type that a value of
    /// `ty` is materialized as by `generate_expression` and stored inline inside a composite.
    /// Read sites that GEP/load an element/field/match-result must size it with THIS
    /// function so the type matches how the value was stored at construction. It differs
    /// from [`type_to_llvm`] in three places:
    ///   - `Array` — an array *value* is the `{ ptr, i64 }` struct `generate_array`
    ///     produces and stores inline (so a nested array `[][]T` keeps that struct as its
    ///     element), whereas `type_to_llvm` lowers `[]T` to a bare opaque pointer.
    ///   - `Record` — a record *value* is a POINTER to its struct (the record ABI:
    ///     `generate_record` returns the alloca), not the struct by value. A `Named` keeps
    ///     the `type_to_llvm` lowering, which already answers by-pointer for a named record
    ///     and the tagged-union struct for a named sum.
    ///   - `Generic` — a payload type variable that survived to a read site (e.g. a match
    ///     whose result type was taken from a never-constructed variant's generic arm)
    ///     has no concrete LLVM type; it falls back to the canonical numeric payload
    ///     representation `f64`, matching how generic/unknown payloads are materialized
    ///     elsewhere (`payload_slot_type`). This keeps such a program compiling (it did
    ///     before the oracle existed) rather than erroring in `type_to_llvm`.
    pub(super) fn value_repr_type(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Array(_) => Ok(self.ptr_len_struct_type().into()),
            Type::Record(_) => Ok(self.context.ptr_type(AddressSpace::default()).into()),
            Type::Generic { .. } => Ok(self.context.f64_type().into()),
            _ => self.type_to_llvm(ty),
        }
    }

    /// The LLVM type a value of `ty` takes when it CROSSES a function boundary — a parameter
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

    /// The value-representation LLVM type to use when GEPing/loading the result of `expression`
    /// (an `arr[i]`, `rec.field`, or `match`), taken from the type oracle. This is the
    /// single read-site policy: ask the oracle for `expression`'s inferred type and lower it
    /// via [`value_repr_type`]; if the oracle has no entry (e.g. the IR-only codegen
    /// tests that skip the type-check pass), fall back to the historical `f64`.
    pub(super) fn oracle_value_type(
        &self,
        expression: &Expression,
    ) -> Result<BasicTypeEnum<'ctx>, String> {
        match self.oracle.expression_type(expression) {
            Some(t) => self.value_repr_type(t),
            None => Ok(self.context.f64_type().into()),
        }
    }

    /// Whether `expression` is a `Text` — the operand test the built-in `Text` operators
    /// (comparison and `+`) route on. Reads the checker's recorded type for the node where
    /// there is one, and only falls back to [`infer_type`]'s structural inference (which
    /// clones) where there is not: the codegen tests that skip the type-check pass.
    pub(super) fn is_text_expression(&self, expression: &Expression) -> bool {
        match self.oracle.expression_type(expression) {
            Some(ty) => *ty == Type::Text,
            None => self.infer_type(expression) == Type::Text,
        }
    }

    /// Best-effort Quilon type of `expression`, sufficient to mangle overloaded call sites.
    /// Codegen lacks the type checker's full inference, so this covers exactly the
    /// shapes that can be an overloaded argument: literals, locals/parameters (tracked in
    /// `var_types`), constructor results, field access on a known record, and the
    /// result types of the supported operators. Falls back to `Num` (the historical
    /// default) when it can't tell — overloaded dispatch then simply won't match and a
    /// clear "function not found" surfaces, never a silent miscompile.
    pub(super) fn infer_type(&self, expression: &Expression) -> Type {
        // Prefer the type checker's authoritative type for this exact node (the oracle) —
        // the same source codegen's read sites use. It knows shapes the structural fallback
        // below can't (an `arr[i]` element, a `.split(…)`/`.replace(…)` result, a field
        // read), so an overloaded call taking one of those (e.g. `assertEq(parts[0], …)`)
        // mangles to the right member. Falls back to structural inference only when the
        // oracle has no entry — the IR-only codegen tests that skip the type-check pass.
        if let Some(ty) = self.oracle.expression_type(expression) {
            return ty.clone();
        }
        match expression {
            Expression::Number { .. } => Type::Num,
            Expression::String { .. } => Type::Text,
            Expression::Bool { .. } => Type::Bool,
            Expression::Unit { .. } => Type::Unit,
            Expression::Identifier { name, .. } => {
                // A bare nullary sum-type constructor (not a bound variable) is a value
                // of its sum type.
                if let Some((_, type_name)) = self.sum_variants.get(name)
                    && !self.var_types.contains_key(name)
                {
                    return self.sum_or_named(type_name);
                }
                self.var_types.get(name).cloned().unwrap_or(Type::Num)
            }
            Expression::Constructor { type_name, .. } => self.sum_or_named(type_name),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if let Expression::Identifier { name, .. } = function.as_ref() {
                    // A constructor call yields its sum type.
                    if let Some((_, type_name)) = self.sum_variants.get(name) {
                        return self.sum_or_named(type_name);
                    }
                    // An overloaded function call yields its resolved member's return.
                    let arg_types: Vec<Type> =
                        arguments.iter().map(|a| self.infer_type(a)).collect();
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
            Expression::BinaryOperator {
                left,
                operator,
                right,
                ..
            } => {
                // A user operator overload yields its resolved member's return type.
                let sym = operator.symbol();
                if self.overloads.contains_key(sym) {
                    let arg_types = [self.infer_type(left), self.infer_type(right)];
                    if let Some((_, ret)) = self.matching_overload(sym, &arg_types) {
                        return ret.clone();
                    }
                }
                // Built-ins: comparisons/logicals yield Bool; `+` on Text yields Text;
                // arithmetic yields Num. Matches the type checker's operator results
                // closely enough to mangle a nested overloaded argument.
                match operator {
                    BinaryOperator::Eq
                    | BinaryOperator::Ne
                    | BinaryOperator::Lt
                    | BinaryOperator::Le
                    | BinaryOperator::Gt
                    | BinaryOperator::Ge
                    | BinaryOperator::And
                    | BinaryOperator::Or => Type::Bool,
                    BinaryOperator::Add
                        if self.infer_type(left) == Type::Text
                            || self.infer_type(right) == Type::Text =>
                    {
                        Type::Text
                    }
                    _ => Type::Num,
                }
            }
            Expression::If { then, .. } => self.infer_type(then),
            // A `?`/`|` match's result type is whatever its arms yield. Codegen can't
            // easily unify the arms, so take the checker's recorded type from the oracle
            // (as record/spread do); this lets a local bound to a match — e.g.
            // `ok = r ? | Ok(_) => true | NotOk(_) => false` — mangle correctly when it
            // later feeds an overloaded call such as `assert(ok)`.
            Expression::Match { .. } => self
                .oracle
                .expression_type(expression)
                .cloned()
                .unwrap_or(Type::Num),
            // Unary `!` is logical-not (Bool); unary `-` is numeric negation (Num). So a
            // local bound to `!ok` mangles as Bool when it feeds an overloaded call.
            Expression::UnaryOperator { operator, .. } => match operator {
                crate::ast::UnaryOperator::Not => Type::Bool,
                crate::ast::UnaryOperator::Neg => Type::Num,
            },
            Expression::Block { statements, .. } => match statements.last() {
                Some(crate::ast::Statement::Expression(tail)) => self.infer_type(tail),
                _ => Type::Num,
            },
            Expression::FieldAccess { field, .. } if field == "size" || field == "length" => {
                Type::Num
            }
            // A record literal / spread — including a functional-update — takes its type
            // from the oracle (which resolves the named-vs-anonymous result of a `<-`
            // spread), so a binding to it mangles / tracks correctly.
            Expression::Record { .. } | Expression::Spread { .. } => self
                .oracle
                .expression_type(expression)
                .cloned()
                .unwrap_or(Type::Num),
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
        let (parameters, _) = self.matching_overload(name, arg_types)?;
        Some(mangle_overload(name, parameters))
    }

    /// The overload member of `name` whose parameter types match `arg_types` exactly
    /// (by type tag), if any. Shared by symbol resolution and return-type inference.
    ///
    /// A member whose LAST parameter is the built-in `Site` also matches one argument short
    /// of it — that parameter takes the caller's location, which the call site fills in (see
    /// `generate_call`). This mirrors the type checker's `resolve_overload`, so both passes
    /// pick the same member.
    pub(super) fn matching_overload(
        &self,
        name: &str,
        arg_types: &[Type],
    ) -> Option<&(Vec<Type>, Type)> {
        self.overloads.get(name)?.iter().find(|(parameters, _)| {
            crate::ast::parameters_accept(parameters, arg_types, |p, a| {
                type_mangle(p) == type_mangle(a)
            })
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
            // A Map/Set VALUE is a single opaque pointer to its runtime representation
            // (a GC-allocated native `HashMap`/`HashSet` wrapper). Validate the element
            // types, but the pointer carries no pointee shape.
            Type::Map(key_type, value_type) => {
                let _k = self.type_to_llvm(key_type)?;
                let _v = self.type_to_llvm(value_type)?;
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Set(elem_type) => {
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
            // function parameter `s :: Shape`). If it names a registered sum type, lower it
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
            // A function-typed value (a closure passed as an argument, or a function-typed
            // parameter) is the `{ ptr fn, ptr env }` closure pair. Validate the parameter
            // and return types, then lower to that shared struct.
            Type::Function {
                parameters,
                return_type,
            } => {
                for parameter in parameters {
                    let _ = self.type_to_llvm(parameter)?;
                }
                let _ = self.type_to_llvm(return_type)?;
                Ok(self.closure_struct_type().into())
            }
            _ => Err(format!("Unsupported type: {:?}", ty)),
        }
    }
}

impl TypeOracle {
    pub(super) fn new(table: crate::typechecker::TypeTable) -> Self {
        Self { table }
    }

    /// The inferred type of `expression`, by its span. `None` if the checker didn't record it.
    pub(super) fn expression_type(&self, expression: &Expression) -> Option<&Type> {
        self.table.get(expression.span())
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
