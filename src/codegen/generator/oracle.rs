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

    /// The Quilon type of a function or lambda parameter: the annotation where one is
    /// written, else the type the checker inferred for it from the receiving signature and
    /// recorded against the parameter's own span. A lambda whose parameters are typed by
    /// context (`apply(10, (n) => n + 1)`) carries nothing in the AST to read, so the
    /// oracle is where its parameter types come from — exactly as read sites already
    /// recover element and field types. `Num` stays the last resort for the IR-only
    /// codegen tests, which build a module without a type-check pass.
    pub(super) fn parameter_type(&self, parameter: &crate::ast::Parameter) -> Type {
        parameter
            .type_annotation
            .clone()
            .or_else(|| self.oracle.type_at(&parameter.span).cloned())
            .unwrap_or(Type::Num)
    }

    /// [`Self::parameter_type`] over a whole parameter list — a signature as the checker
    /// resolved it.
    pub(super) fn parameter_types(&self, parameters: &[crate::ast::Parameter]) -> Vec<Type> {
        parameters.iter().map(|p| self.parameter_type(p)).collect()
    }

    /// Whether `expression` is a `Text` — the operand test the built-in `Text` operators
    /// (comparison and `+`) route on. `None` (an expression the checker never typed) reads
    /// as "not Text", exactly like the array/set checks this routing sits alongside.
    pub(super) fn is_text_expression(&self, expression: &Expression) -> bool {
        self.oracle.expression_type(expression) == Some(&Type::Text)
    }

    /// The checker's recorded type for `expression` — every codegen read of "what type is
    /// this" that isn't already a parameter/element/field type it holds directly funnels
    /// through here. `description` names what is being lowered, for the error a missing
    /// entry raises: a missing entry is a compiler bug (the checker records one for every
    /// expression it type-checks), never a fallback to a guess.
    pub(super) fn oracle_type(
        &self,
        expression: &Expression,
        description: &str,
    ) -> Result<Type, String> {
        self.oracle
            .expression_type(expression)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no type recorded for {description} at {:?} — it was not type-checked",
                    expression.span()
                )
            })
    }

    /// [`Self::oracle_type`] over a call's arguments, in the order an overloaded call's
    /// dispatch reads them to pick its member.
    pub(super) fn oracle_argument_types(
        &self,
        arguments: &[Expression],
    ) -> Result<Vec<Type>, String> {
        arguments
            .iter()
            .map(|a| self.oracle_type(a, "an overloaded call's argument"))
            .collect()
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

    /// The type the checker recorded for a source span. The one lookup into the table;
    /// a parameter is not an expression and has no node of its own, so its inferred type
    /// is read back by its own span.
    pub(super) fn type_at(&self, span: &Span) -> Option<&Type> {
        self.table.get(span)
    }

    /// The inferred type of `expression`, by its span. `None` if the checker didn't record it.
    pub(super) fn expression_type(&self, expression: &Expression) -> Option<&Type> {
        self.type_at(expression.span())
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
