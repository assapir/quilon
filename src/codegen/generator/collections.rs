//! Code generation for the built-in `Map` and `Set` collection types.
//!
//! A Map/Set value is a single opaque pointer to a GC-allocated native runtime wrapper
//! (a `std::collections::HashMap`/`HashSet` with a fixed-seed hasher; see
//! `quilon-rt/src/collections.rs`). The collections are IMMUTABLE — every mutator
//! (`set`/`add`, the set operators) returns a NEW collection pointer and never touches
//! the receiver.
//!
//! Keys/elements are passed across the runtime ABI as a uniform triple `(tag, a, b)` of
//! `i64`s: `tag` picks the hashable kind (0 = Num, 1 = Text, 2 = Bool); `a`/`b` carry the
//! bits (a Num's f64 bits, a Bool's 0/1, or a Text's data pointer + byte length). Values
//! are boxed on the GC heap and stored as an opaque pointer, loaded back at the value's
//! static type — so a Map/Set can hold values of any type while the runtime stays generic.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state.

use super::*;
use inkwell::values::{BasicMetadataValueEnum, IntValue};

/// The key-kind tags shared with the runtime ABI (`quilon-rt`'s `QlKeyTag`).
const KEY_TAG_NUM: u64 = 0;
const KEY_TAG_TEXT: u64 = 1;
const KEY_TAG_BOOL: u64 = 2;

impl<'ctx> CodeGenerator<'ctx> {
    // ---- literals ---------------------------------------------------------

    /// `[|k1 => v1, ...|]` — build a fresh map by `__map_new` then a persistent
    /// `__map_set` per entry. The oracle gives the map's `Map(K, V)` type.
    pub(super) fn generate_map_literal(
        &mut self,
        node: &Expr,
        entries: &[(Expr, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (key_ty, value_ty) = match self.oracle.expr_type(node) {
            Some(Type::Map(k, v)) => ((**k).clone(), (**v).clone()),
            _ => return Err("map literal has no Map type in the oracle".to_string()),
        };
        let value_llvm = self.value_repr_type(&value_ty)?;

        let mut map = self.call_rt_ptr("__map_new", &[])?;
        for (key_expr, value_expr) in entries {
            let (tag, ka, kb) = self.key_words(key_expr, &key_ty)?;
            let value = self.generate_expr(value_expr)?;
            let boxed = self.box_value(value, value_llvm)?;
            map = self.call_rt_ptr(
                "__map_set",
                &[map.into(), tag.into(), ka.into(), kb.into(), boxed.into()],
            )?;
        }
        Ok(map.into())
    }

    /// `[|e1, e2, ...|]` — build a fresh set by `__set_new` then `__set_add` per element.
    pub(super) fn generate_set_literal(
        &mut self,
        node: &Expr,
        elements: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let elem_ty = match self.oracle.expr_type(node) {
            Some(Type::Set(e)) => (**e).clone(),
            _ => return Err("set literal has no Set type in the oracle".to_string()),
        };
        let mut set = self.call_rt_ptr("__set_new", &[])?;
        for elem in elements {
            let (tag, a, b) = self.key_words(elem, &elem_ty)?;
            set = self.call_rt_ptr("__set_add", &[set.into(), tag.into(), a.into(), b.into()])?;
        }
        Ok(set.into())
    }

    // ---- indexing ---------------------------------------------------------

    /// `m[k]` — fail-loud keyed lookup. `__map_index` returns the value box pointer or
    /// crashes (stderr + exit 1) when the key is absent; codegen loads the value at its
    /// static type.
    pub(super) fn generate_map_index(
        &mut self,
        index_node: &Expr,
        map_expr: &Expr,
        key_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let key_ty = match self.oracle.expr_type(map_expr) {
            Some(Type::Map(k, _)) => (**k).clone(),
            _ => return Err("indexed value is not a Map".to_string()),
        };
        let map = self.generate_expr(map_expr)?.into_pointer_value();
        let (tag, ka, kb) = self.key_words(key_expr, &key_ty)?;
        let boxed = self.call_rt_ptr(
            "__map_index",
            &[map.into(), tag.into(), ka.into(), kb.into()],
        )?;
        let value_llvm = self.oracle_value_type(index_node)?;
        self.builder
            .build_load(value_llvm, boxed, "map_val")
            .map_err(ctx("Failed to load map value"))
    }

    // ---- map methods ------------------------------------------------------

    pub(super) fn generate_map_method(
        &mut self,
        method: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (key_ty, value_ty) = match self.oracle.expr_type(&args[0]) {
            Some(Type::Map(k, v)) => ((**k).clone(), (**v).clone()),
            _ => return Err("map method receiver is not a Map".to_string()),
        };
        let value_llvm = self.value_repr_type(&value_ty)?;
        let map = self.generate_expr(&args[0])?.into_pointer_value();

        match method {
            "has" => {
                let (tag, ka, kb) = self.key_words(&args[1], &key_ty)?;
                let found =
                    self.call_rt_int("__map_has", &[map.into(), tag.into(), ka.into(), kb.into()])?;
                self.int_to_bool(found)
            }
            "set" => {
                let (tag, ka, kb) = self.key_words(&args[1], &key_ty)?;
                let value = self.generate_expr(&args[2])?;
                let boxed = self.box_value(value, value_llvm)?;
                let out = self.call_rt_ptr(
                    "__map_set",
                    &[map.into(), tag.into(), ka.into(), kb.into(), boxed.into()],
                )?;
                Ok(out.into())
            }
            "get" => self.generate_map_get(map, &args[1], &key_ty, value_llvm),
            "keys" => self.build_key_array(map, &key_ty),
            "values" => self.build_values_array(map, value_llvm),
            "each" => {
                self.map_each(map, &args[1], &key_ty, value_llvm, &value_ty)?;
                Ok(map.into())
            }
            other => Err(format!("unhandled map method {other}")),
        }
    }

    /// `m.get(k)` — the safe lookup: `Ok(v)` when present, else `NotOk`. Mirrors
    /// `array_at`'s Result construction.
    fn generate_map_get(
        &mut self,
        map: PointerValue<'ctx>,
        key_expr: &Expr,
        key_ty: &Type,
        value_llvm: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (tag, ka, kb) = self.key_words(key_expr, key_ty)?;
        let i64t = self.context.i64_type();
        let found_slot = self.create_entry_block_alloca("mget_found", i64t.into())?;
        let boxed = self.call_rt_ptr(
            "__map_get",
            &[
                map.into(),
                tag.into(),
                ka.into(),
                kb.into(),
                found_slot.into(),
            ],
        )?;
        let found = self
            .builder
            .build_load(i64t, found_slot, "mget_found_v")
            .map_err(ctx("Failed to load found flag"))?
            .into_int_value();
        let found_bool = self
            .builder
            .build_int_compare(inkwell::IntPredicate::NE, found, i64t.const_zero(), "mget_hit")
            .map_err(ctx("Failed to test found flag"))?;

        let function = self
            .current_function
            .ok_or_else(|| "map.get outside of function".to_string())?;
        let result_ty = self.result_struct_type(value_llvm);
        let result_ptr = self.create_entry_block_alloca("mget_result", result_ty.into())?;
        let ok_bb = self.context.append_basic_block(function, "mget_ok");
        let no_bb = self.context.append_basic_block(function, "mget_no");
        let cont_bb = self.context.append_basic_block(function, "mget_cont");
        self.builder
            .build_conditional_branch(found_bool, ok_bb, no_bb)
            .map_err(ctx("Failed to branch on map.get"))?;

        self.builder.position_at_end(ok_bb);
        let value = self
            .builder
            .build_load(value_llvm, boxed, "mget_val")
            .map_err(ctx("Failed to load map value"))?;
        let ok = self.build_result(value_llvm, "Ok", value)?;
        self.builder
            .build_store(result_ptr, ok)
            .map_err(ctx("Failed to store Ok"))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(ctx("Failed to branch to cont"))?;

        self.builder.position_at_end(no_bb);
        let no = self.build_result(value_llvm, "NotOk", zeroed(value_llvm))?;
        self.builder
            .build_store(result_ptr, no)
            .map_err(ctx("Failed to store NotOk"))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(ctx("Failed to branch to cont"))?;

        self.builder.position_at_end(cont_bb);
        self.builder
            .build_load(result_ty, result_ptr, "mget_out")
            .map_err(ctx("Failed to load map.get result"))
    }

    /// `m.keys()` — a `[]K` array in the runtime's (unspecified) iteration order.
    fn build_key_array(
        &mut self,
        map: PointerValue<'ctx>,
        key_ty: &Type,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let key_llvm = self.value_repr_type(key_ty)?;
        let n = self.call_rt_int("__map_len", &[map.into()])?;
        let out_ptr = self.alloc_array_data(key_llvm, n)?;
        let key_ty = key_ty.clone();
        self.array_loop(n, |this, i| {
            let ka = this.call_rt_int("__map_key_a", &[map.into(), i.into()])?;
            let kb = this.call_rt_int("__map_key_b", &[map.into(), i.into()])?;
            let key = this.key_from_words(ka, kb, &key_ty)?;
            let dst = unsafe {
                this.builder
                    .build_gep(key_llvm, out_ptr, &[i], "keys_dst")
                    .map_err(ctx("Failed to GEP keys dst"))?
            };
            this.builder
                .build_store(dst, key)
                .map_err(ctx("Failed to store key"))?;
            Ok(())
        })?;
        self.array_struct(out_ptr, n)
    }

    /// `m.values()` — a `[]V` array in the same order as `keys()`.
    fn build_values_array(
        &mut self,
        map: PointerValue<'ctx>,
        value_llvm: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let n = self.call_rt_int("__map_len", &[map.into()])?;
        let out_ptr = self.alloc_array_data(value_llvm, n)?;
        self.array_loop(n, |this, i| {
            let boxed = this.call_rt_ptr("__map_val", &[map.into(), i.into()])?;
            let value = this
                .builder
                .build_load(value_llvm, boxed, "vals_v")
                .map_err(ctx("Failed to load value"))?;
            let dst = unsafe {
                this.builder
                    .build_gep(value_llvm, out_ptr, &[i], "vals_dst")
                    .map_err(ctx("Failed to GEP values dst"))?
            };
            this.builder
                .build_store(dst, value)
                .map_err(ctx("Failed to store value"))?;
            Ok(())
        })?;
        self.array_struct(out_ptr, n)
    }

    /// `m.each(f)` — inline `f(key, value)` over every entry, for effect.
    fn map_each(
        &mut self,
        map: PointerValue<'ctx>,
        lambda: &Expr,
        key_ty: &Type,
        value_llvm: BasicTypeEnum<'ctx>,
        value_ty: &Type,
    ) -> Result<(), String> {
        let n = self.call_rt_int("__map_len", &[map.into()])?;
        let key_ty = key_ty.clone();
        let value_ty = value_ty.clone();
        self.array_loop(n, |this, i| {
            let ka = this.call_rt_int("__map_key_a", &[map.into(), i.into()])?;
            let kb = this.call_rt_int("__map_key_b", &[map.into(), i.into()])?;
            let key = this.key_from_words(ka, kb, &key_ty)?;
            let boxed = this.call_rt_ptr("__map_val", &[map.into(), i.into()])?;
            let value = this
                .builder
                .build_load(value_llvm, boxed, "each_v")
                .map_err(ctx("Failed to load value"))?;
            this.inline_lambda(lambda, &[(key, key_ty.clone()), (value, value_ty.clone())])?;
            Ok(())
        })
    }

    // ---- set methods ------------------------------------------------------

    pub(super) fn generate_set_method(
        &mut self,
        method: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let elem_ty = match self.oracle.expr_type(&args[0]) {
            Some(Type::Set(e)) => (**e).clone(),
            _ => return Err("set method receiver is not a Set".to_string()),
        };
        let set = self.generate_expr(&args[0])?.into_pointer_value();

        match method {
            "has" => {
                let (tag, a, b) = self.key_words(&args[1], &elem_ty)?;
                let found =
                    self.call_rt_int("__set_has", &[set.into(), tag.into(), a.into(), b.into()])?;
                self.int_to_bool(found)
            }
            "add" => {
                let (tag, a, b) = self.key_words(&args[1], &elem_ty)?;
                let out =
                    self.call_rt_ptr("__set_add", &[set.into(), tag.into(), a.into(), b.into()])?;
                Ok(out.into())
            }
            "items" => self.build_items_array(set, &elem_ty),
            "each" => {
                self.set_each(set, &args[1], &elem_ty)?;
                Ok(set.into())
            }
            other => Err(format!("unhandled set method {other}")),
        }
    }

    /// `s.items()` — a `[]T` array in the runtime's (unspecified) iteration order.
    fn build_items_array(
        &mut self,
        set: PointerValue<'ctx>,
        elem_ty: &Type,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let elem_llvm = self.value_repr_type(elem_ty)?;
        let n = self.call_rt_int("__set_len", &[set.into()])?;
        let out_ptr = self.alloc_array_data(elem_llvm, n)?;
        let elem_ty = elem_ty.clone();
        self.array_loop(n, |this, i| {
            let a = this.call_rt_int("__set_item_a", &[set.into(), i.into()])?;
            let b = this.call_rt_int("__set_item_b", &[set.into(), i.into()])?;
            let elem = this.key_from_words(a, b, &elem_ty)?;
            let dst = unsafe {
                this.builder
                    .build_gep(elem_llvm, out_ptr, &[i], "items_dst")
                    .map_err(ctx("Failed to GEP items dst"))?
            };
            this.builder
                .build_store(dst, elem)
                .map_err(ctx("Failed to store item"))?;
            Ok(())
        })?;
        self.array_struct(out_ptr, n)
    }

    /// `s.each(f)` — inline `f(elem)` over every element, for effect.
    fn set_each(
        &mut self,
        set: PointerValue<'ctx>,
        lambda: &Expr,
        elem_ty: &Type,
    ) -> Result<(), String> {
        let n = self.call_rt_int("__set_len", &[set.into()])?;
        let elem_ty = elem_ty.clone();
        self.array_loop(n, |this, i| {
            let a = this.call_rt_int("__set_item_a", &[set.into(), i.into()])?;
            let b = this.call_rt_int("__set_item_b", &[set.into(), i.into()])?;
            let elem = this.key_from_words(a, b, &elem_ty)?;
            this.inline_lambda(lambda, &[(elem, elem_ty.clone())])?;
            Ok(())
        })
    }

    // ---- set operators ----------------------------------------------------

    /// `+` union, `-` difference, `+-`/`-+` intersection — each returns a NEW set.
    pub(super) fn generate_set_op(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let intrinsic = match op {
            BinOp::Add => "__set_union",
            BinOp::Sub => "__set_diff",
            BinOp::SetIntersect => "__set_intersect",
            _ => return Err(format!("{op:?} is not a set operator")),
        };
        let l = self.generate_expr(left)?.into_pointer_value();
        let r = self.generate_expr(right)?.into_pointer_value();
        let out = self.call_rt_ptr(intrinsic, &[l.into(), r.into()])?;
        Ok(out.into())
    }

    // ---- key encoding / boxing helpers ------------------------------------

    /// Lower a key/element expression to the runtime ABI triple `(tag, a, b)` of `i64`s.
    fn key_words(
        &mut self,
        key_expr: &Expr,
        key_ty: &Type,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>, IntValue<'ctx>), String> {
        let i64t = self.context.i64_type();
        let value = self.generate_expr(key_expr)?;
        match key_ty {
            Type::Num => {
                let bits = self
                    .builder
                    .build_bit_cast(value.into_float_value(), i64t, "key_num_bits")
                    .map_err(ctx("Failed to bitcast Num key"))?
                    .into_int_value();
                Ok((i64t.const_int(KEY_TAG_NUM, false), bits, i64t.const_zero()))
            }
            Type::Bool => {
                let ext = self
                    .builder
                    .build_int_z_extend(value.into_int_value(), i64t, "key_bool")
                    .map_err(ctx("Failed to extend Bool key"))?;
                Ok((i64t.const_int(KEY_TAG_BOOL, false), ext, i64t.const_zero()))
            }
            Type::Text => {
                let s = value.into_struct_value();
                let data = self
                    .builder
                    .build_extract_value(s, 0, "key_text_ptr")
                    .map_err(ctx("Failed to extract Text ptr"))?
                    .into_pointer_value();
                let len = self
                    .builder
                    .build_extract_value(s, 1, "key_text_len")
                    .map_err(ctx("Failed to extract Text len"))?
                    .into_int_value();
                let data_int = self
                    .builder
                    .build_ptr_to_int(data, i64t, "key_text_addr")
                    .map_err(ctx("Failed to ptrtoint Text key"))?;
                Ok((i64t.const_int(KEY_TAG_TEXT, false), data_int, len))
            }
            other => Err(format!(
                "unsupported map/set key type: {}",
                crate::ast::type_label(other)
            )),
        }
    }

    /// Reconstruct a key/element value (in its value representation) from the runtime ABI
    /// words `a`/`b` produced by `key_words`.
    fn key_from_words(
        &mut self,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
        key_ty: &Type,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match key_ty {
            Type::Num => self
                .builder
                .build_bit_cast(a, self.context.f64_type(), "key_num")
                .map_err(ctx("Failed to bitcast Num key back")),
            Type::Bool => Ok(self
                .builder
                .build_int_truncate(a, self.context.bool_type(), "key_bool")
                .map_err(ctx("Failed to truncate Bool key"))?
                .into()),
            Type::Text => {
                let ptr = self
                    .builder
                    .build_int_to_ptr(
                        a,
                        self.context.ptr_type(AddressSpace::default()),
                        "key_text_ptr",
                    )
                    .map_err(ctx("Failed to inttoptr Text key"))?;
                let text_ty = self.ptr_len_struct_type();
                let with_ptr = self
                    .builder
                    .build_insert_value(text_ty.get_undef(), ptr, 0, "key_text_p")
                    .map_err(ctx("Failed to insert Text ptr"))?
                    .into_struct_value();
                let text = self
                    .builder
                    .build_insert_value(with_ptr, b, 1, "key_text_l")
                    .map_err(ctx("Failed to insert Text len"))?
                    .into_struct_value();
                Ok(text.into())
            }
            other => Err(format!(
                "unsupported map/set key type: {}",
                crate::ast::type_label(other)
            )),
        }
    }

    /// GC-allocate a box holding one value and store `value` into it; returns the box
    /// pointer stored in the native table (loaded back at the value's static type).
    fn box_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        value_llvm: BasicTypeEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let one = self.context.i64_type().const_int(1, false);
        let boxed = self.alloc_array_data(value_llvm, one)?;
        self.builder
            .build_store(boxed, value)
            .map_err(ctx("Failed to store boxed value"))?;
        Ok(boxed)
    }

    /// Convert a runtime `i64` truthiness flag (0/1) to a Quilon `Bool` (`i1`).
    fn int_to_bool(&mut self, flag: IntValue<'ctx>) -> Result<BasicValueEnum<'ctx>, String> {
        Ok(self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                flag,
                self.context.i64_type().const_zero(),
                "rt_bool",
            )
            .map_err(ctx("Failed to convert flag to Bool"))?
            .into())
    }

    // ---- runtime-call plumbing --------------------------------------------

    /// Call a runtime intrinsic returning an opaque pointer (a Map/Set/box pointer).
    fn call_rt_ptr(
        &mut self,
        name: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<PointerValue<'ctx>, String> {
        let f = self.get_intrinsic(name)?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(f, args, "rt_call")
            .map_err(|e| format!("Failed to call {name}: {e:?}"))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// Call a runtime intrinsic returning an `i64`.
    fn call_rt_int(
        &mut self,
        name: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<IntValue<'ctx>, String> {
        let f = self.get_intrinsic(name)?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(f, args, "rt_call")
            .map_err(|e| format!("Failed to call {name}: {e:?}"))?
            .as_any_value_enum()
            .into_int_value())
    }
}
