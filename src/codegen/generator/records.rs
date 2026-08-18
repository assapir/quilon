//! Records: literals, functional update (`<-` spread), field reads and in-place field
//! writes, and the struct layout fields are addressed through.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;
use std::rc::Rc;

impl<'ctx> CodeGenerator<'ctx> {
    /// Reorder a constructor call's `fields` into the named type's DECLARATION order so
    /// the lowered struct's slot order matches what `record_types` and the type oracle
    /// use to index fields. Falls back to the provided order if the type's field list
    /// isn't registered. (The expressions are cloned — constructor field lists are tiny.)
    pub(super) fn constructor_fields_in_decl_order(
        &self,
        type_name: &str,
        fields: &[(String, Expr)],
    ) -> Vec<(String, Expr)> {
        let Some(decl_order) = self.named_type_fields.get(type_name) else {
            return fields.to_vec();
        };
        decl_order
            .iter()
            .filter_map(|fname| {
                fields
                    .iter()
                    .find(|(provided, _)| provided == fname)
                    .cloned()
            })
            .collect()
    }

    /// Lower a record literal, routing a functional-update literal (`{<-p, x = 9}`,
    /// containing one or more `<-` spreads) to [`generate_record_update`] and an ordinary
    /// literal to [`generate_record`].
    pub(super) fn generate_record_expr(
        &mut self,
        record_expr: &Expr,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if fields.iter().any(|(_, v)| matches!(v, Expr::Spread { .. })) {
            self.generate_record_update(record_expr, fields)
        } else {
            self.generate_record(fields)
        }
    }

    /// Lower a record functional-update `{<-p, x = 9, ...}`: build a NEW record whose
    /// field set / order / types come from the whole literal's oracle type (a `Named`
    /// type keeps its declared layout and methods; otherwise it is an anonymous record).
    /// Each result field's value is the explicit override if the literal supplies one
    /// (`x = 9`), else the field copied from the LAST spread source that carries it —
    /// so later entries override earlier ones, left-to-right.
    pub(super) fn generate_record_update(
        &mut self,
        record_expr: &Expr,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if self.current_function.is_none() {
            return Err("Global records not yet implemented".to_string());
        }

        // Result layout (ordered fields + types) from the oracle — authoritative for both
        // the struct shape and which slot each name occupies.
        let result_fields: Rc<Vec<(String, Type)>> = match self.oracle.expr_type(record_expr) {
            Some(Type::Named { fields, .. }) => Rc::clone(fields),
            Some(Type::Record(fields)) => Rc::new(fields.clone()),
            _ => {
                return Err(
                    "record functional-update requires type information (missing oracle entry)"
                        .to_string(),
                );
            }
        };

        // Evaluate the literal's parts in source order (left-to-right), recording for each
        // field name its LATEST provider — so precedence follows source order exactly:
        // a later entry (override OR spread) beats an earlier one, an override beats an
        // earlier spread, and a later spread beats an earlier override. (Splitting on
        // "override vs spread" instead would wrongly make an explicit field always win
        // regardless of position, e.g. `{x = 9, <-p}` must yield `p.x`, not `9`.)
        enum Provider<'v> {
            Override(BasicValueEnum<'v>),
            Spread(usize), // index into `sources`
        }
        struct Source<'v> {
            ptr: PointerValue<'v>,
            layout: Rc<Vec<(String, Type)>>,
            // The source record's LLVM struct type, reconstructed once here (not per
            // field copied from it) so field GEPs just index it.
            struct_type: inkwell::types::StructType<'v>,
        }
        let mut sources: Vec<Source<'ctx>> = Vec::new();
        let mut provider: HashMap<String, Provider<'ctx>> = HashMap::new();

        for (name, value) in fields {
            if let Expr::Spread { expr: src, .. } = value {
                let layout: Rc<Vec<(String, Type)>> = match self.oracle.expr_type(src) {
                    Some(Type::Named { fields, .. }) => Rc::clone(fields),
                    Some(Type::Record(fields)) => Rc::new(fields.clone()),
                    _ => {
                        return Err("record spread source requires type information".to_string());
                    }
                };
                let fnames: Vec<String> = layout.iter().map(|(n, _)| n.clone()).collect();
                let struct_type = self.record_struct_type(&layout)?;
                let ptr = self.generate_expr(src)?.into_pointer_value();
                let idx = sources.len();
                sources.push(Source {
                    ptr,
                    layout,
                    struct_type,
                });
                for fname in fnames {
                    provider.insert(fname, Provider::Spread(idx));
                }
            } else {
                let v = self.generate_expr(value)?;
                provider.insert(name.clone(), Provider::Override(v));
            }
        }

        // Result field repr types, computed once — reused both to load copied fields and
        // to build the result struct (matching how `record_field_pointer` reconstructs it
        // later). The struct is GC-allocated so it may escape the frame.
        let field_types: Vec<BasicTypeEnum> = result_fields
            .iter()
            .map(|(_, t)| self.value_repr_type(t))
            .collect::<Result<Vec<_>, _>>()?;

        // Assemble each result field's value in result (slot) order.
        let mut field_values: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(result_fields.len());
        for (i, (fname, _)) in result_fields.iter().enumerate() {
            let field_llvm = field_types[i];
            match provider.get(fname) {
                Some(Provider::Override(v)) => field_values.push(*v),
                Some(Provider::Spread(si)) => {
                    // Copy the field out of its providing spread source.
                    let src = &sources[*si];
                    let idx = src
                        .layout
                        .iter()
                        .position(|(n, _)| n == fname)
                        .ok_or_else(|| format!("spread source missing field {}", fname))?;
                    let gep = self
                        .builder
                        .build_struct_gep(src.struct_type, src.ptr, idx as u32, "spread_field_ptr")
                        .map_err(ctx("Failed to GEP spread field"))?;
                    let loaded = self
                        .builder
                        .build_load(field_llvm, gep, fname)
                        .map_err(ctx("Failed to load spread field"))?;
                    field_values.push(loaded);
                }
                None => {
                    return Err(format!(
                        "record functional-update result field {fname} has no source"
                    ));
                }
            }
        }

        let struct_type = self.context.struct_type(&field_types, false);
        use inkwell::values::AnyValue;
        let size = struct_type
            .size_of()
            .ok_or_else(|| "record struct type has no compile-time size".to_string())?;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let record_ptr = self
            .builder
            .build_call(alloc_fn, &[size.into()], "record")
            .map_err(ctx("Failed to call __alloc for record"))?
            .as_any_value_enum()
            .into_pointer_value();
        for (i, value) in field_values.iter().enumerate() {
            let gep = self
                .builder
                .build_struct_gep(struct_type, record_ptr, i as u32, &format!("field_{}", i))
                .map_err(ctx("Failed to build GEP"))?;
            self.builder
                .build_store(gep, *value)
                .map_err(ctx("Failed to build store"))?;
        }
        Ok(record_ptr.into())
    }

    pub(super) fn generate_record(
        &mut self,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if fields.is_empty() {
            // Empty record - create empty struct
            let struct_type = self.context.struct_type(&[], false);
            return Ok(struct_type.const_zero().into());
        }

        // Generate all field values
        let mut field_values: Vec<BasicValueEnum> = Vec::new();
        for (_name, expr) in fields {
            field_values.push(self.generate_expr(expr)?);
        }

        // Get field types
        let field_types: Vec<BasicTypeEnum> = field_values.iter().map(|v| v.get_type()).collect();

        // Create struct type
        let struct_type = self.context.struct_type(&field_types, false);

        // Create the struct value
        if self.current_function.is_some() {
            // GC-allocate the struct (not a stack alloca) so a record VALUE can outlive
            // the frame that built it — e.g. a record returned from a function or a user
            // operator overload (`+ = (a :: Vec, b :: Vec) -> Vec => Vec { ... }`). A
            // stack alloca would dangle once the callee returned.
            use inkwell::values::AnyValue;
            let size = struct_type
                .size_of()
                .ok_or_else(|| "record struct type has no compile-time size".to_string())?;
            let alloc_fn = self.get_intrinsic("__alloc")?;
            let record_ptr = self
                .builder
                .build_call(alloc_fn, &[size.into()], "record")
                .map_err(ctx("Failed to call __alloc for record"))?
                .as_any_value_enum()
                .into_pointer_value();

            // Store each field
            for (i, value) in field_values.iter().enumerate() {
                let gep = self
                    .builder
                    .build_struct_gep(struct_type, record_ptr, i as u32, &format!("field_{}", i))
                    .map_err(ctx("Failed to build GEP"))?;
                self.builder
                    .build_store(gep, *value)
                    .map_err(ctx("Failed to build store"))?;
            }

            Ok(record_ptr.into())
        } else {
            // For globals, we need constant values
            Err("Global records not yet implemented".to_string())
        }
    }

    pub(super) fn generate_field_access(
        &mut self,
        expr: &Expr,
        field_name: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A record may legitimately have a field literally named `size`/`length`.
        // Resolve known record fields by NAME first (matching the type checker,
        // which dispatches on static type) so they don't collide with the Text/array
        // `.size`/`.length` struct-shape handling below. Text/array values are never
        // tracked in `record_types`, so this only diverts genuine record fields.
        let is_named_record_field = matches!(expr, Expr::Ident { name, .. }
            if self
                .record_types
                .get(name)
                .is_some_and(|fields| fields.iter().any(|f| f == field_name)));

        // A Map/Set `.size` is the runtime entry/element count (`__map_len`/`__set_len`);
        // a map/set value is an opaque pointer, so dispatch on the oracle's receiver type
        // before the array/Text struct-shape handling below (see `generate_collection_size`).
        if !is_named_record_field && field_name == "size" {
            let count_intrinsic = match self.oracle.expr_type(expr) {
                Some(Type::Map(_, _)) => Some("__map_len"),
                Some(Type::Set(_)) => Some("__set_len"),
                _ => None,
            };
            if let Some(intrinsic) = count_intrinsic {
                return self.generate_collection_size(expr, intrinsic);
            }
        }

        // Special handling for .size field on arrays
        if !is_named_record_field && field_name == "size" {
            // For arrays (which are structs {ptr, i64}), we need special handling
            // Check if it's an identifier - we can directly work with the alloca
            if let Expr::Ident { name, .. } = expr
                && let Some((var_ptr, var_type)) = self.variables.get(name).cloned()
            {
                // Check if this is a struct type (could be an array)
                if let BasicTypeEnum::StructType(struct_type) = var_type {
                    // Get field 1 (size field of array struct) directly from the alloca
                    let size_field = self
                        .builder
                        .build_struct_gep(struct_type, var_ptr, 1, "size_field")
                        .map_err(ctx("Failed to get size field"))?;

                    let size_val = self
                        .builder
                        .build_load(self.context.i64_type(), size_field, "size")
                        .map_err(ctx("Failed to load size"))?;

                    // Convert i64 to f64 (Num)
                    if let BasicValueEnum::IntValue(i) = size_val {
                        let size_f64 = self
                            .builder
                            .build_signed_int_to_float(i, self.context.f64_type(), "size_as_num")
                            .map_err(ctx("Failed to convert size"))?;

                        return Ok(size_f64.into());
                    }
                }
            }
        }

        // Text/array as a value: `.size` is the i64 length field (byte length for
        // Text); `.length` is the grapheme count (Text only — the checker rejects
        // `.length` on arrays). Handles non-identifier receivers like `("a"+"b").size`.
        if !is_named_record_field && (field_name == "size" || field_name == "length") {
            let val = self.generate_expr(expr)?;
            if let BasicValueEnum::StructValue(s) = val {
                let len = self
                    .builder
                    .build_extract_value(s, 1, "len_field")
                    .map_err(ctx("Failed to extract length field"))?
                    .into_int_value();
                if field_name == "size" {
                    return Ok(self
                        .builder
                        .build_signed_int_to_float(len, self.context.f64_type(), "size_as_num")
                        .map_err(ctx("Failed to convert size"))?
                        .into());
                }
                // `.length`: grapheme-cluster count via __text_length(data, byte_len).
                let data = self
                    .builder
                    .build_extract_value(s, 0, "data_field")
                    .map_err(ctx("Failed to extract data field"))?
                    .into_pointer_value();
                let len_fn = self.get_intrinsic("__text_length")?;
                use inkwell::values::AnyValue;
                let count = self
                    .builder
                    .build_call(len_fn, &[data.into(), len.into()], "graphemes")
                    .map_err(ctx("Failed to call __text_length"))?
                    .as_any_value_enum()
                    .into_int_value();
                return Ok(self
                    .builder
                    .build_signed_int_to_float(count, self.context.f64_type(), "length_as_num")
                    .map_err(ctx("Failed to convert length"))?
                    .into());
            }
        }

        // Regular record field access: resolve a pointer to the field inside the
        // record's memory (shared by the in-place field-write path) and load it with the
        // field's declared LLVM type from the oracle (NOT a hardcoded `f64`), so a
        // `Text`/array field reads back correctly.
        if let Some((field_ptr, field_llvm)) = self.record_field_pointer(expr, field_name)? {
            return self
                .builder
                .build_load(field_llvm, field_ptr, field_name)
                .map_err(ctx("Failed to load field"));
        }

        Err(format!(
            "Field access not fully implemented. Need type information for field '{}'",
            field_name
        ))
    }

    /// In-place field write `target := value`, where `target` is a field access
    /// `obj.field`. Computes a pointer into the existing record memory via GEP and
    /// stores `value` there — no re-allocation — so the mutation is observable
    /// through every alias of the record. Yields `$` (a unit i8), matching the
    /// type checker's `Unit` result for a field write.
    pub(super) fn generate_field_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Expr::FieldAccess { expr, field, .. } = target else {
            return Err("Field-write target must be a field access".to_string());
        };
        let new_value = self.generate_expr(value)?;
        let (field_ptr, _field_llvm) = self
            .record_field_pointer(expr, field)?
            .ok_or_else(|| format!("Unknown record for field write: {}", field))?;
        self.builder
            .build_store(field_ptr, new_value)
            .map_err(ctx("Failed to store field"))?;
        Ok(self.unit_value().into())
    }

    /// Pointer to `base.field` inside the record's memory, plus the field's value-repr
    /// LLVM type — the shared primitive for both reads (`generate_field_access`) and
    /// in-place writes (`generate_field_assign`).
    ///
    /// `base` must be a record/named-type identifier (a variable such as `u`, or the
    /// method receiver `it`); the variable's alloca holds a pointer-to-struct (the
    /// record ABI). The struct's field types are recovered from the **type oracle** (the
    /// record's declared field types), mapped through `value_repr_type` so the
    /// reconstructed struct type matches exactly how `generate_record` laid it out —
    /// `Text`/array/etc. fields keep their real type instead of being treated as `f64`.
    /// The returned LLVM type is what the read site must `load` (and the write site is
    /// already type-checked to match).
    ///
    /// Nested records (`a.b.c`) are rejected by the type checker before codegen, so a
    /// single GEP level suffices. Returns `Ok(None)` when `base` isn't a tracked record
    /// (so the read path can fall through to its Text/array `.size` handling).
    pub(super) fn record_field_pointer(
        &mut self,
        base: &Expr,
        field: &str,
    ) -> Result<Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>, String> {
        let Expr::Ident { name, .. } = base else {
            return Ok(None);
        };
        let Some(field_names) = self.record_types.get(name).cloned() else {
            return Ok(None);
        };
        let Some(field_idx) = field_names.iter().position(|f| f == field) else {
            return Ok(None);
        };

        // Reconstruct the record's struct type from the oracle (its declared field types,
        // in declared order) via the shared `record_struct_type`, so the GEP type matches
        // construction. Fall back to all-`f64` only if the oracle has no record type for
        // `base` (it always should for a tracked record) — preserving the historical
        // numeric layout. The loaded field's own LLVM type is then just the indexed slot.
        let struct_type = match self.oracle.expr_type(base) {
            // Cloned out of the oracle (an `Rc` bump for a named type, whose declaration
            // is shared) so the borrow ends before `record_struct_type` takes `&self`.
            Some(Type::Record(fields)) => {
                let fields = fields.clone();
                self.record_struct_type(&fields)?
            }
            Some(Type::Named { fields, .. }) => {
                let fields = Rc::clone(fields);
                self.record_struct_type(&fields)?
            }
            _ => {
                let f64t: BasicTypeEnum = self.context.f64_type().into();
                self.context
                    .struct_type(&vec![f64t; field_names.len()], false)
            }
        };
        let field_llvm = struct_type
            .get_field_type_at_index(field_idx as u32)
            .ok_or_else(|| format!("record field index {field_idx} out of range"))?;

        // The variable's alloca holds a pointer to the struct; load it.
        let (var_ptr, _) = self
            .variables
            .get(name)
            .ok_or_else(|| format!("Variable not found: {}", name))?;
        let struct_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                *var_ptr,
                "load_struct_ptr",
            )
            .map_err(ctx("Failed to load struct pointer"))?
            .into_pointer_value();

        let field_ptr = self
            .builder
            .build_struct_gep(
                struct_type,
                struct_ptr,
                field_idx as u32,
                &format!("field_{}_ptr", field),
            )
            .map_err(ctx("Failed to build field GEP"))?;
        Ok(Some((field_ptr, field_llvm)))
    }

    /// The LLVM struct type for a record with the given (name, Quilon-type) fields, in
    /// declared order — each slot lowered through [`value_repr_type`]. This is the single
    /// definition of a record's memory layout, shared by record construction
    /// (`generate_record_update`) and field reads (`record_field_pointer`), so the two
    /// can never disagree on slot types.
    pub(super) fn record_struct_type(
        &self,
        fields: &[(String, Type)],
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        let field_types: Vec<BasicTypeEnum> = fields
            .iter()
            .map(|(_, t)| self.value_repr_type(t))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.context.struct_type(&field_types, false))
    }
}
