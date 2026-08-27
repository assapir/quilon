//! Code generation for the built-in `Map` and `Set` collection types.
//!
//! A Map/Set value is a single opaque pointer to a GC-allocated native runtime wrapper
//! (a `std::collections::HashMap`/`HashSet` with a fixed-seed hasher; see
//! `quilon-rt/src/collections/`). The collections are IMMUTABLE — every mutator
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

/// The key-kind tags shared with the runtime ABI (`quilon-rt`'s `TAG_*`
/// in `collections/common.rs`).
const KEY_TAG_NUM: u64 = 0;
const KEY_TAG_TEXT: u64 = 1;
const KEY_TAG_BOOL: u64 = 2;
const KEY_TAG_USER: u64 = 3;

/// A key/element lowered to the runtime ABI: the triple `(tag, a, b)` of `i64`s plus the
/// `%` hash and `==` function pointers a `TAG_USER` key hashes/compares through (both null
/// for a built-in Num/Text/Bool key).
struct KeyAbi<'ctx> {
    tag: IntValue<'ctx>,
    a: IntValue<'ctx>,
    b: IntValue<'ctx>,
    hash_fn: PointerValue<'ctx>,
    eq_fn: PointerValue<'ctx>,
}

impl<'ctx> KeyAbi<'ctx> {
    /// A runtime key-op call's argument list: the `leading` collection pointer, the five key
    /// ABI words `(tag, a, b, hash_fn, eq_fn)`, then any `trailing` arguments (a boxed value
    /// for `set`, the found-out slot for `get`).
    fn call_arguments(
        &self,
        leading: BasicMetadataValueEnum<'ctx>,
        trailing: &[BasicMetadataValueEnum<'ctx>],
    ) -> Vec<BasicMetadataValueEnum<'ctx>> {
        let mut arguments = vec![
            leading,
            self.tag.into(),
            self.a.into(),
            self.b.into(),
            self.hash_fn.into(),
            self.eq_fn.into(),
        ];
        arguments.extend_from_slice(trailing);
        arguments
    }
}

impl<'ctx> CodeGenerator<'ctx> {
    /// `[|k1 => v1, ...|]` — build a fresh map by `__map_new` then a persistent
    /// `__map_set` per entry. The oracle gives the map's `Map(K, V)` type.
    pub(super) fn generate_map_literal(
        &mut self,
        node: &Expression,
        entries: &[(Expression, Expression)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (key_ty, value_ty) = match self.oracle.expression_type(node) {
            Some(Type::Map(k, v)) => ((**k).clone(), (**v).clone()),
            _ => return Err("map literal has no Map type in the oracle".to_string()),
        };
        let value_llvm = self.value_repr_type(&value_ty)?;

        let mut map = self.call_rt_ptr("__map_new", &[])?;
        for (key_expression, value_expression) in entries {
            let key = self.key_abi(key_expression, &key_ty)?;
            let value = self.generate_expression(value_expression)?;
            let boxed = self.box_value(value, value_llvm)?;
            map = self.call_rt_ptr(
                "__map_set",
                &key.call_arguments(map.into(), &[boxed.into()]),
            )?;
        }
        Ok(map.into())
    }

    /// `[|e1, e2, ...|]` — build a fresh set by `__set_new` then `__set_add` per element.
    pub(super) fn generate_set_literal(
        &mut self,
        node: &Expression,
        elements: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let elem_ty = match self.oracle.expression_type(node) {
            Some(Type::Set(e)) => (**e).clone(),
            _ => return Err("set literal has no Set type in the oracle".to_string()),
        };
        let mut set = self.call_rt_ptr("__set_new", &[])?;
        for elem in elements {
            let key = self.key_abi(elem, &elem_ty)?;
            set = self.call_rt_ptr("__set_add", &key.call_arguments(set.into(), &[]))?;
        }
        Ok(set.into())
    }

    pub(super) fn generate_map_method(
        &mut self,
        method: &str,
        arguments: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (key_ty, value_ty) = match self.oracle.expression_type(&arguments[0]) {
            Some(Type::Map(k, v)) => ((**k).clone(), (**v).clone()),
            _ => return Err("map method receiver is not a Map".to_string()),
        };
        let value_llvm = self.value_repr_type(&value_ty)?;
        let map = self
            .generate_expression(&arguments[0])?
            .into_pointer_value();

        match method {
            "has" => {
                let key = self.key_abi(&arguments[1], &key_ty)?;
                let found = self.call_rt_int("__map_has", &key.call_arguments(map.into(), &[]))?;
                self.int_to_bool(found, "rt_bool")
            }
            "set" => {
                let key = self.key_abi(&arguments[1], &key_ty)?;
                let value = self.generate_expression(&arguments[2])?;
                let boxed = self.box_value(value, value_llvm)?;
                let out = self.call_rt_ptr(
                    "__map_set",
                    &key.call_arguments(map.into(), &[boxed.into()]),
                )?;
                Ok(out.into())
            }
            "remove" => {
                let key = self.key_abi(&arguments[1], &key_ty)?;
                let out = self.call_rt_ptr("__map_remove", &key.call_arguments(map.into(), &[]))?;
                Ok(out.into())
            }
            "get" => self.generate_map_get(map, &arguments[1], &key_ty, value_llvm),
            "keys" => self.build_key_array(map, &key_ty),
            "values" => self.build_values_array(map, value_llvm),
            "each" => {
                self.map_each(map, &arguments[1], &key_ty, value_llvm, &value_ty)?;
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
        key_expression: &Expression,
        key_ty: &Type,
        value_llvm: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let key = self.key_abi(key_expression, key_ty)?;
        let i64t = self.context.i64_type();
        let found_slot = self.create_entry_block_alloca("mget_found", i64t.into())?;
        let boxed = self.call_rt_ptr(
            "__map_get",
            &key.call_arguments(map.into(), &[found_slot.into()]),
        )?;
        let found = self
            .builder
            .build_load(i64t, found_slot, "mget_found_v")
            .map_err(ctx("Failed to load found flag"))?
            .into_int_value();
        let found_bool = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                found,
                i64t.const_zero(),
                "mget_hit",
            )
            .map_err(ctx("Failed to test found flag"))?;

        // The value box is dereferenced only on the found branch (the `Ok` payload), so a
        // null return on a miss is never loaded.
        self.build_conditional_result(found_bool, value_llvm, "mget", |this| {
            this.builder
                .build_load(value_llvm, boxed, "mget_val")
                .map_err(ctx("Failed to load map value"))
        })
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
        lambda: &Expression,
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

    pub(super) fn generate_set_method(
        &mut self,
        method: &str,
        arguments: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let elem_ty = match self.oracle.expression_type(&arguments[0]) {
            Some(Type::Set(e)) => (**e).clone(),
            _ => return Err("set method receiver is not a Set".to_string()),
        };
        let set = self
            .generate_expression(&arguments[0])?
            .into_pointer_value();

        match method {
            "has" => {
                let key = self.key_abi(&arguments[1], &elem_ty)?;
                let found = self.call_rt_int("__set_has", &key.call_arguments(set.into(), &[]))?;
                self.int_to_bool(found, "rt_bool")
            }
            "add" => {
                let key = self.key_abi(&arguments[1], &elem_ty)?;
                let out = self.call_rt_ptr("__set_add", &key.call_arguments(set.into(), &[]))?;
                Ok(out.into())
            }
            "remove" => {
                let key = self.key_abi(&arguments[1], &elem_ty)?;
                let out = self.call_rt_ptr("__set_remove", &key.call_arguments(set.into(), &[]))?;
                Ok(out.into())
            }
            "items" => self.build_items_array(set, &elem_ty),
            "each" => {
                self.set_each(set, &arguments[1], &elem_ty)?;
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
        lambda: &Expression,
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

    /// Emit a Map/Set `.size` field read (the entry/element count as a `Num`) via
    /// `intrinsic` (`__map_len` / `__set_len`). Shared by `generate_field_access`.
    pub(super) fn generate_collection_size(
        &mut self,
        receiver: &Expression,
        intrinsic: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let collection = self.generate_expression(receiver)?.into_pointer_value();
        let len = self.call_rt_int(intrinsic, &[collection.into()])?;
        Ok(self
            .builder
            .build_signed_int_to_float(len, self.context.f64_type(), "size_as_num")
            .map_err(ctx("Failed to convert size"))?
            .into())
    }

    /// `+` union, `-` difference, `+-`/`-+` intersection — each returns a NEW set.
    pub(super) fn generate_set_op(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let intrinsic = match operator {
            BinaryOperator::Add => "__set_union",
            BinaryOperator::Sub => "__set_diff",
            BinaryOperator::SetIntersect => "__set_intersect",
            _ => return Err(format!("{operator:?} is not a set operator")),
        };
        let l = self.generate_expression(left)?.into_pointer_value();
        let r = self.generate_expression(right)?.into_pointer_value();
        let out = self.call_rt_ptr(intrinsic, &[l.into(), r.into()])?;
        Ok(out.into())
    }

    /// Lower a key/element expression to the runtime ABI: the triple `(tag, a, b)` plus the
    /// `%`/`==` function pointers (null for a built-in key). A user key type boxes its value
    /// on the GC heap and passes the box pointer as `a`, so the runtime hashes/compares it
    /// through the type's monomorphized `%`/`==` via those pointers.
    fn key_abi(
        &mut self,
        key_expression: &Expression,
        key_ty: &Type,
    ) -> Result<KeyAbi<'ctx>, String> {
        let i64t = self.context.i64_type();
        let null = self.context.ptr_type(AddressSpace::default()).const_null();
        let value = self.generate_expression(key_expression)?;
        match key_ty {
            Type::Num => {
                let bits = self
                    .builder
                    .build_bit_cast(value.into_float_value(), i64t, "key_num_bits")
                    .map_err(ctx("Failed to bitcast Num key"))?
                    .into_int_value();
                Ok(KeyAbi {
                    tag: i64t.const_int(KEY_TAG_NUM, false),
                    a: bits,
                    b: i64t.const_zero(),
                    hash_fn: null,
                    eq_fn: null,
                })
            }
            Type::Bool => {
                let ext = self
                    .builder
                    .build_int_z_extend(value.into_int_value(), i64t, "key_bool")
                    .map_err(ctx("Failed to extend Bool key"))?;
                Ok(KeyAbi {
                    tag: i64t.const_int(KEY_TAG_BOOL, false),
                    a: ext,
                    b: i64t.const_zero(),
                    hash_fn: null,
                    eq_fn: null,
                })
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
                Ok(KeyAbi {
                    tag: i64t.const_int(KEY_TAG_TEXT, false),
                    a: data_int,
                    b: len,
                    hash_fn: null,
                    eq_fn: null,
                })
            }
            Type::Named { name, .. } | Type::Sum { name, .. } => {
                let value_llvm = self.value_repr_type(key_ty)?;
                let boxed = self.box_value(value, value_llvm)?;
                let addr = self
                    .builder
                    .build_ptr_to_int(boxed, i64t, "key_user_addr")
                    .map_err(ctx("Failed to ptrtoint user key"))?;
                let (hash_fn, eq_fn) = self.user_key_trampolines(name, key_ty)?;
                Ok(KeyAbi {
                    tag: i64t.const_int(KEY_TAG_USER, false),
                    a: addr,
                    b: i64t.const_zero(),
                    hash_fn,
                    eq_fn,
                })
            }
            other => Err(format!(
                "unsupported map/set key type: {}",
                crate::ast::type_label(other)
            )),
        }
    }

    /// Reconstruct a key/element value (in its value representation) from the runtime ABI
    /// words `a`/`b` produced by `key_abi`.
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
            Type::Named { .. } | Type::Sum { .. } => {
                // `a` is the boxed-key pointer; load the value at its representation (a
                // record's by-pointer, a sum's by-value struct).
                let value_llvm = self.value_repr_type(key_ty)?;
                let box_ptr = self
                    .builder
                    .build_int_to_ptr(
                        a,
                        self.context.ptr_type(AddressSpace::default()),
                        "key_user_box",
                    )
                    .map_err(ctx("Failed to inttoptr user key"))?;
                self.builder
                    .build_load(value_llvm, box_ptr, "key_user")
                    .map_err(ctx("Failed to load user key"))
            }
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

    /// The `%` hash and `==` function pointers a user key type crosses the runtime ABI
    /// with, emitting the per-type trampolines once and reusing them thereafter.
    fn user_key_trampolines(
        &mut self,
        type_name: &str,
        key_ty: &Type,
    ) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>), String> {
        let hash_name = format!("__keyhash_{type_name}");
        let eq_name = format!("__keyeq_{type_name}");
        if self.module.get_function(&hash_name).is_none() {
            self.emit_key_trampolines(key_ty, &hash_name, &eq_name)?;
        }
        let hash_fn = self
            .module
            .get_function(&hash_name)
            .unwrap()
            .as_global_value()
            .as_pointer_value();
        let eq_fn = self
            .module
            .get_function(&eq_name)
            .unwrap()
            .as_global_value()
            .as_pointer_value();
        Ok((hash_fn, eq_fn))
    }

    /// Emit the two trampolines a user key type `K` is hashed/compared through: `keyhash`
    /// loads the boxed key and returns its `%` `Num` hash; `keyeq` loads two boxed keys and
    /// returns `K`'s `==` as an `i64` (normalizing the member's `Bool`/`i1` across the C ABI).
    /// Both call the monomorphized operator members `#178` emits for `K`.
    fn emit_key_trampolines(
        &mut self,
        key_ty: &Type,
        hash_name: &str,
        eq_name: &str,
    ) -> Result<(), String> {
        use inkwell::values::AnyValue;

        // The value representation the box holds and the member expects `it` as: a record's
        // by-pointer, a sum's by-value struct.
        let value_llvm = self.value_repr_type(key_ty)?;
        let ptr = self.context.ptr_type(AddressSpace::default());
        let f64t = self.context.f64_type();
        let i64t = self.context.i64_type();

        let hash_symbol = mangle_overload("%", std::slice::from_ref(key_ty));
        let hash_member = self
            .module
            .get_function(&hash_symbol)
            .ok_or_else(|| format!("key type has no `%` hash member ({hash_symbol}) emitted"))?;
        let eq_symbol = mangle_overload("==", &[key_ty.clone(), key_ty.clone()]);
        let eq_member = self
            .module
            .get_function(&eq_symbol)
            .ok_or_else(|| format!("key type has no `==` member ({eq_symbol}) emitted"))?;

        // Emitting fresh functions mid-stream: save and restore the enclosing builder
        // position and debug location so the surrounding function body resumes intact.
        let saved_block = self.builder.get_insert_block();
        let saved_function = self.current_function;
        let saved_loc = self.builder.get_current_debug_location();
        self.builder.unset_current_debug_location();

        let hash_fn = self
            .module
            .add_function(hash_name, f64t.fn_type(&[ptr.into()], false), None);
        hash_fn.set_linkage(inkwell::module::Linkage::Internal);
        self.current_function = Some(hash_fn);
        let entry = self.context.append_basic_block(hash_fn, "entry");
        self.builder.position_at_end(entry);
        let box_ptr = hash_fn.get_nth_param(0).unwrap().into_pointer_value();
        let it = self
            .builder
            .build_load(value_llvm, box_ptr, "it")
            .map_err(ctx("Failed to load boxed key"))?;
        let hashed = self
            .builder
            .build_call(hash_member, &[it.into()], "keyhash")
            .map_err(ctx("Failed to call `%` member"))?
            .as_any_value_enum()
            .into_float_value();
        self.builder
            .build_return(Some(&hashed))
            .map_err(ctx("Failed to return key hash"))?;

        let eq_fn = self.module.add_function(
            eq_name,
            i64t.fn_type(&[ptr.into(), ptr.into()], false),
            None,
        );
        eq_fn.set_linkage(inkwell::module::Linkage::Internal);
        self.current_function = Some(eq_fn);
        let entry = self.context.append_basic_block(eq_fn, "entry");
        self.builder.position_at_end(entry);
        let left_box = eq_fn.get_nth_param(0).unwrap().into_pointer_value();
        let right_box = eq_fn.get_nth_param(1).unwrap().into_pointer_value();
        let left = self
            .builder
            .build_load(value_llvm, left_box, "left")
            .map_err(ctx("Failed to load left key"))?;
        let right = self
            .builder
            .build_load(value_llvm, right_box, "right")
            .map_err(ctx("Failed to load right key"))?;
        let equal = self
            .builder
            .build_call(eq_member, &[left.into(), right.into()], "keyeq")
            .map_err(ctx("Failed to call `==` member"))?
            .as_any_value_enum()
            .into_int_value();
        let widened = self
            .builder
            .build_int_z_extend(equal, i64t, "keyeq64")
            .map_err(ctx("Failed to widen key equality"))?;
        self.builder
            .build_return(Some(&widened))
            .map_err(ctx("Failed to return key equality"))?;

        self.current_function = saved_function;
        if let Some(block) = saved_block {
            self.builder.position_at_end(block);
        }
        if let Some(loc) = saved_loc {
            self.builder.set_current_debug_location(loc);
        }
        Ok(())
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

    /// Call a runtime intrinsic returning an opaque pointer (a Map/Set/box pointer).
    fn call_rt_ptr(
        &mut self,
        name: &str,
        arguments: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<PointerValue<'ctx>, String> {
        let f = self.get_intrinsic(name)?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(f, arguments, "rt_call")
            .map_err(|e| format!("Failed to call {name}: {e:?}"))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// Call a runtime intrinsic returning an `i64`.
    pub(super) fn call_rt_int(
        &mut self,
        name: &str,
        arguments: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<IntValue<'ctx>, String> {
        let f = self.get_intrinsic(name)?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(f, arguments, "rt_call")
            .map_err(|e| format!("Failed to call {name}: {e:?}"))?
            .as_any_value_enum()
            .into_int_value())
    }
}
