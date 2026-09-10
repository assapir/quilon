//! Sum types as tagged unions: constructing a variant, and the struct layouts that
//! carry a tag plus its payload.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;
use inkwell::types::StructType;

/// A sum type's payload layout, decided once from its variants by [`positions_agree`] —
/// shared by LLVM struct construction (this module) and the DWARF builder (`di.rs`), so
/// the decision and the resulting shape can never drift between the two.
///
/// `PerPosition` is today's shape: one canonical slot per payload position, sized to
/// whichever variant's field there is widest (`payload_slot_types`) — kept whenever every
/// variant's concrete field at a position agrees, which is the common case (`Result`, and
/// most user sums) and the smaller of the two struct shapes.
///
/// `Union` is Rust-enum style: each variant's OWN fields form their own LLVM struct
/// (`variant_bodies`), and the sum's shared storage is `widest` — whichever of those
/// struct types is largest. A narrower variant's value is WRITTEN as its own struct type
/// and READ BACK as `widest` (`generate_union_sum_value`); a match arm does the reverse
/// (`union_payload_values`). LLVM's opaque pointers make this a plain memory round-trip:
/// a `store`/`load` pair needs no shared type at the pointer itself, only enough allocated
/// bytes and alignment, which `widest` — the biggest of the candidates — guarantees.
#[derive(Clone)]
pub(super) enum SumLayout<'ctx> {
    PerPosition(Vec<BasicTypeEnum<'ctx>>),
    Union {
        variant_bodies: HashMap<String, StructType<'ctx>>,
        widest: StructType<'ctx>,
    },
}

/// Whether every variant's concrete (non-`$`, non-generic) field at each payload
/// position agrees in type with every other variant's there — the condition under which
/// a sum keeps ONE canonical slot per position (`SumLayout::PerPosition`); anything else
/// needs the union layout. The single decision both the LLVM struct builder
/// (`build_sum_layout`) and the DWARF builder (`di.rs::di_sum_type`) make, so a sum's
/// debug-info shape can never disagree with the value it describes.
pub(super) fn positions_agree(variants: &[crate::ast::SumVariant]) -> bool {
    let arity = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
    (0..arity).all(|position| {
        let mut concrete: Option<&Type> = None;
        variants
            .iter()
            .all(|variant| match variant.fields.get(position) {
                Some(field) if *field != Type::Unit && !matches!(field, Type::Generic { .. }) => {
                    match concrete {
                        None => {
                            concrete = Some(field);
                            true
                        }
                        Some(prev) => prev == field,
                    }
                }
                _ => true,
            })
    })
}

/// The C-ABI byte size and alignment LLVM gives `ty` on every architecture Quilon
/// targets (all LP64: 8-byte pointers/doubles, natural alignment, no packing) — used
/// only to pick the WIDEST variant body for the union layout, never to build a value.
fn static_layout(ty: BasicTypeEnum) -> (u64, u64) {
    match ty {
        BasicTypeEnum::FloatType(_) | BasicTypeEnum::PointerType(_) => (8, 8),
        BasicTypeEnum::IntType(int_type) => {
            let bytes = u64::from(int_type.get_bit_width()).div_ceil(8).max(1);
            (bytes, bytes)
        }
        BasicTypeEnum::StructType(struct_type) => {
            let mut offset = 0u64;
            let mut align = 1u64;
            for index in 0..struct_type.count_fields() {
                let field = struct_type
                    .get_field_type_at_index(index)
                    .expect("index within count_fields");
                let (field_size, field_align) = static_layout(field);
                align = align.max(field_align);
                offset = offset.next_multiple_of(field_align) + field_size;
            }
            (offset.next_multiple_of(align), align)
        }
        BasicTypeEnum::ArrayType(array_type) => {
            let (elem_size, elem_align) = static_layout(array_type.get_element_type());
            (elem_size * u64::from(array_type.len()), elem_align)
        }
        BasicTypeEnum::VectorType(_) | BasicTypeEnum::ScalableVectorType(_) => (8, 8),
    }
}

impl<'ctx> CodeGenerator<'ctx> {
    /// Decide and build `variants`' payload layout (see [`SumLayout`]). A field naming the
    /// enclosing sum directly lowers through the ordinary [`Self::value_repr_type`] path,
    /// which — since this sum's own name isn't in `sum_layouts` yet while its layout is
    /// still being computed — falls back to a plain pointer, exactly the GC-boxed
    /// representation [`Self::coerce_payload`]/[`Self::unbox_if_boxed`] expect. This
    /// relies on the caller (`register_sum_variants`) inserting into `sum_layouts` only
    /// AFTER this returns; inserting a partial entry before calling this would turn a
    /// self-reference into a wrongly-typed struct instead of the intended pointer.
    pub(super) fn build_sum_layout(
        &self,
        variants: &[crate::ast::SumVariant],
    ) -> Result<SumLayout<'ctx>, String> {
        if positions_agree(variants) {
            return Ok(SumLayout::PerPosition(self.payload_slot_types(variants)?));
        }
        let mut variant_bodies = HashMap::with_capacity(variants.len());
        let mut widest: Option<StructType<'ctx>> = None;
        let mut widest_size = 0u64;
        for variant in variants {
            let field_types: Vec<BasicTypeEnum> = variant
                .fields
                .iter()
                .map(|field| self.value_repr_type(field))
                .collect::<Result<_, _>>()?;
            let body = self.context.struct_type(&field_types, false);
            let (size, _) = static_layout(body.into());
            if size >= widest_size {
                widest = Some(body);
                widest_size = size;
            }
            variant_bodies.insert(variant.name.clone(), body);
        }
        Ok(SumLayout::Union {
            widest: widest.unwrap_or_else(|| self.context.struct_type(&[], false)),
            variant_bodies,
        })
    }

    pub(super) fn generate_sum_constructor(
        &mut self,
        tag: u8,
        type_name: &str,
        variant: &str,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i8_type = self.context.i8_type();
        let tag_val = i8_type.const_int(tag as u64, false);

        // One lookup decides which shape this sum's values take (see `SumLayout`): a
        // `Union` layout builds and returns immediately, below; a `PerPosition` one (or
        // an unregistered name, e.g. an IR-only test) carries its slots on to the
        // tagged-union construction that follows.
        let registered_layout = match self.sum_layouts.get(type_name) {
            Some(SumLayout::Union {
                variant_bodies,
                widest,
            }) => {
                let body_ty = *variant_bodies
                    .get(variant)
                    .ok_or_else(|| format!("unregistered variant `{variant}`"))?;
                let widest = *widest;
                return self.generate_union_sum_value(type_name, tag_val, body_ty, widest, args);
            }
            Some(SumLayout::PerPosition(slots)) => Some(slots.clone()),
            None => None,
        };

        // Tagged-union value: { i8 tag, slot0, slot1, ... }. Every sum type has a registered
        // canonical layout (`sum_layouts`), so EVERY value of the type shares one struct shape
        // and a match arm can extract any variant's slots without going out of range:
        //  - USER sum types are sized to the widest variant, one slot per payload position:
        //      Rect(3, 4) -> { i8 1, double 3.0, double 4.0 }
        //      Circle(9)  -> { i8 0, double 9.0, double <undef> }   (slot 1 unused)
        //  - `Result` has ONE canonical `{ptr,i64}` slot into which ANY payload is PACKED
        //    (`pack_result_payload`), so its heterogeneous, generic variants still share the
        //    single shape `{ i8, {ptr,i64} }`:
        //      Ok(42)       -> { i8 0, {ptr,i64} {null, <42.0 bits>} }
        //      NotOk("err") -> { i8 1, {ptr,i64} <the Text struct> }
        //
        // For USER slots, Num/Bool payloads are normalized to f64 and a `$` (Unit) payload is
        // stored as a zero of the slot type so the value still matches the slot/return shape
        // (e.g. `Ok($)` packs a zeroed slot) — the bits are never read.

        // Determine each payload slot's value. `Result` packs its payload into the one
        // canonical `{ptr,i64}` slot; a user type's slot type is fixed by position from its
        // registered layout (the `None` arms below only fire for the unregistered-name
        // fallback, e.g. an IR-only test that skips declaration).
        let is_result = type_name == "Result";
        let mut payload_vals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
        for (pos, arg) in args.iter().enumerate() {
            let arg_val = self.generate_expression(arg)?;
            if is_result {
                // Result has a single canonical `{ptr,i64}` slot; PACK the payload into it
                // (a Text/array fills it directly, a scalar goes into one field) so every
                // Result shares the `{ i8, {ptr,i64} }` shape regardless of payload.
                payload_vals.push(self.pack_result_payload(arg_val)?);
                continue;
            }
            // With a registered layout (user type), the slot type is fixed by position.
            // Without one, the slot follows the value's own type so a Text/Bool payload
            // keeps its real representation — `payload_slot_type` already widens a `$`
            // (Unit) value's `i8` to the canonical `double` slot, same as any other
            // non-Bool integer.
            let slot_ty = match registered_layout.as_ref().and_then(|l| l.get(pos).copied()) {
                Some(ty) => ty,
                None => self.payload_slot_type(arg_val),
            };
            payload_vals.push(self.coerce_payload(arg_val, slot_ty)?);
        }

        // Build the struct type: tag + (registered layout, or the actual payload types).
        let mut field_types: Vec<BasicTypeEnum> = vec![i8_type.into()];
        match &registered_layout {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.extend(payload_vals.iter().map(|v| v.get_type())),
        }
        let sum_struct = self.context.struct_type(&field_types, false);

        let mut agg = sum_struct.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "with_tag")
            .map_err(ctx("Failed to insert tag"))?
            .into_struct_value();
        // Fill the leading slots with this variant's payloads; trailing slots (unused by
        // this variant, in a wider registered layout) stay `undef` — they're only read by
        // an arm matching a different, wider variant, which never runs for this value.
        for (i, payload) in payload_vals.iter().enumerate() {
            agg = self
                .builder
                .build_insert_value(agg, *payload, (i + 1) as u32, "with_payload")
                .map_err(ctx("Failed to insert payload"))?
                .into_struct_value();
        }

        Ok(agg.into())
    }

    /// Reinterpret struct `value` as `target`'s type through a memory round-trip: an
    /// entry-block alloca sized for `alloc_ty` (the wider of the two structs in play, by
    /// construction — see `SumLayout::Union`), a `store` of `value` at its own type, then
    /// a `load` back at `target`'s. LLVM's opaque pointers need no shared type at the
    /// pointer itself for this, only enough allocated bytes and alignment. Shared by
    /// [`Self::generate_union_sum_value`] (widening a variant's own body up to the sum's
    /// shared storage) and [`Self::union_payload_values`] (narrowing that storage back
    /// down to one variant's own view).
    fn reinterpret_struct(
        &mut self,
        value: inkwell::values::StructValue<'ctx>,
        alloc_ty: StructType<'ctx>,
        target: StructType<'ctx>,
        label: &str,
    ) -> Result<inkwell::values::StructValue<'ctx>, String> {
        let storage = self.create_entry_block_alloca(label, alloc_ty.into())?;
        self.builder
            .build_store(storage, value)
            .map_err(ctx("Failed to store sum payload"))?;
        self.builder
            .build_load(target, storage, label)
            .map_err(ctx("Failed to reinterpret sum payload"))
            .map(|v| v.into_struct_value())
    }

    /// Build a `SumLayout::Union` value: this variant's own fields packed into `body_ty`
    /// (its own struct type), then widened to the shared `widest` storage by a memory
    /// round-trip — `store` as `body_ty`, `load` back as `widest` — before tagging.
    fn generate_union_sum_value(
        &mut self,
        type_name: &str,
        tag_val: inkwell::values::IntValue<'ctx>,
        body_ty: StructType<'ctx>,
        widest: StructType<'ctx>,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let mut field_vals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            let arg_val = self.generate_expression(arg)?;
            let slot_ty = body_ty
                .get_field_type_at_index(i as u32)
                .ok_or_else(|| format!("payload slot {i} out of range"))?;
            field_vals.push(self.coerce_payload(arg_val, slot_ty)?);
        }
        let mut body = body_ty.get_undef();
        for (i, value) in field_vals.into_iter().enumerate() {
            body = self
                .builder
                .build_insert_value(body, value, i as u32, "with_field")
                .map_err(ctx("Failed to insert union payload field"))?
                .into_struct_value();
        }
        let widened = self.reinterpret_struct(body, widest, widest, "sum_body_widened")?;

        let sum_struct = self.sum_struct_type(type_name);
        let mut agg = sum_struct.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "with_tag")
            .map_err(ctx("Failed to insert tag"))?
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, widened, 1, "with_body")
            .map_err(ctx("Failed to insert payload"))?
            .into_struct_value();
        Ok(agg.into())
    }

    /// Raw (possibly still-boxed) payload values for `variant`'s fields out of a matched
    /// `struct_val` of sum type `type_name`. A `PerPosition` sum reads its payload
    /// directly out of the tagged struct's own slots (positions `1..`); a `Union` sum
    /// reads the shared `widest` body back through a memory round-trip AS the variant's
    /// own (narrower) struct type — the mirror of [`Self::generate_union_sum_value`].
    pub(super) fn union_payload_values(
        &mut self,
        struct_val: inkwell::values::StructValue<'ctx>,
        type_name: &str,
        variant: &str,
        arity: usize,
    ) -> Result<Option<Vec<BasicValueEnum<'ctx>>>, String> {
        let body_ty = match self.sum_layouts.get(type_name) {
            Some(SumLayout::Union { variant_bodies, .. }) => *variant_bodies
                .get(variant)
                .ok_or_else(|| format!("unregistered variant `{variant}`"))?,
            _ => return Ok(None),
        };
        let widened = self
            .builder
            .build_extract_value(struct_val, 1, "sum_body")
            .map_err(ctx("Failed to extract sum payload"))?
            .into_struct_value();
        let widest = widened.get_type();
        let narrow = self.reinterpret_struct(widened, widest, body_ty, "sum_body_narrow")?;
        (0..arity)
            .map(|i| {
                self.builder
                    .build_extract_value(narrow, i as u32, "payload")
                    .map_err(ctx("Failed to extract payload field"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// The slot type for a Result payload sized to its actual value: a non-`i1` integer
    /// widens to f64 (the canonical numeric payload), everything else keeps its own type.
    pub(super) fn payload_slot_type(&self, value: BasicValueEnum<'ctx>) -> BasicTypeEnum<'ctx> {
        match value {
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() != 1 => {
                self.context.f64_type().into()
            }
            other => other.get_type(),
        }
    }

    /// Coerce a payload argument value to its target slot type. Integers (incl. the unit
    /// `i8`) widen to f64 for a numeric slot; a `$` (Unit) value targeting a non-`i8` slot
    /// becomes a zero of that slot type (it carries no information). Otherwise the value
    /// is stored as-is (e.g. a Text struct into a Text slot).
    pub(super) fn coerce_payload(
        &self,
        value: BasicValueEnum<'ctx>,
        slot_ty: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) if slot_ty.is_float_type() => Ok(self
                .builder
                .build_unsigned_int_to_float(i, slot_ty.into_float_type(), "inttofloat")
                .map_err(ctx("Failed to convert payload to float"))?
                .into()),
            // A value already matching the slot type passes through unchanged.
            other if other.get_type() == slot_ty => Ok(other),
            // A `$` (Unit) value — the zero `i8` — carries no information; stored into a
            // differently-typed slot it becomes that slot's zero (e.g. a `$` payload in a
            // `Done($) / Pending(Text)` Text slot). The type checker guarantees concrete
            // payload types agree per position, so ANY other mismatch is an internal bug,
            // surfaced rather than silently zeroed.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 8 => Ok(zeroed(slot_ty)),
            // A pointer-typed slot given the sum's own by-value aggregate: the field
            // names the enclosing sum directly (`Node(Tree)`), which — since that sum
            // isn't registered in `sum_layouts` yet while ITS OWN layout is being built —
            // lowered to a plain pointer rather than the recursive struct it can't be.
            // GC-box the value and slot the pointer; `unbox_if_boxed` reverses this when
            // a pattern binds it. The only payload this arm ever sees: a record is
            // already a pointer at construction (never reaches here as a struct value),
            // and a Map/Set value is always an opaque pointer too.
            BasicValueEnum::StructValue(_) if slot_ty.is_pointer_type() => {
                let box_ptr = self.alloc_box(value.get_type())?;
                self.builder
                    .build_store(box_ptr, value)
                    .map_err(ctx("Failed to box sum payload"))?;
                Ok(box_ptr.into())
            }
            other => Err(format!(
                "internal error: sum-type payload of type {:?} does not fit slot {:?}",
                other.get_type(),
                slot_ty
            )),
        }
    }

    /// Undo [`Self::coerce_payload`]'s box: a `ptr` payload whose bind-site target type is
    /// a sum loads back through the GC cell as that sum's ordinary by-value aggregate —
    /// the representation every other value of the type has. A `ptr` target (a record, or
    /// any other pointer-represented type) is not boxed and passes through unchanged.
    pub(super) fn unbox_if_boxed(
        &self,
        raw: BasicValueEnum<'ctx>,
        target: Option<&Type>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (BasicValueEnum::PointerValue(ptr), Some(target_ty)) = (raw, target) else {
            return Ok(raw);
        };
        let repr = self.value_repr_type(target_ty)?;
        if repr.is_pointer_type() {
            return Ok(raw);
        }
        self.builder
            .build_load(repr, ptr, "unboxed_payload")
            .map_err(ctx("Failed to unbox payload"))
    }

    /// Pack a Result payload `value` into the canonical `{ptr,i64}` slot, so any payload —
    /// scalar or composite — shares one LLVM shape. The reverse of [`unpack_result_payload`],
    /// which must read back the SAME concrete type this packed:
    ///   - `Text` / array (already `{ptr,i64}`): stored directly as the whole slot.
    ///   - `Num` (f64): its bits go into field `.1` (bitcast to i64), `.0` = null.
    ///   - `Bool` (i1): zero-extended into field `.1`, `.0` = null.
    ///   - `$` (Unit, a zero `i8`) / anything else scalar: a zeroed slot.
    ///   - a record pointer: stored into field `.0`, `.1` = 0.
    ///   - any other aggregate wider than the slot (a user sum value, a nested `Result`, a
    ///     closure): stored in a GC box, whose pointer rides in field `.0`.
    pub(super) fn pack_result_payload(
        &self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let slot_ty = self.ptr_len_struct_type();
        // A `{ptr,i64}` value (Text or array) already IS the slot.
        if value.get_type() == slot_ty.into() {
            return Ok(value);
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let (ptr_field, int_field) = match value {
            // A record/opaque pointer payload rides in the pointer field.
            BasicValueEnum::PointerValue(p) => (p, i64_ty.const_zero()),
            // A Num (f64) payload: reinterpret its bits as i64 in the integer field.
            BasicValueEnum::FloatValue(f) => {
                let bits = self
                    .builder
                    .build_bit_cast(f, i64_ty, "num_bits")
                    .map_err(ctx("Failed to bitcast Num payload"))?
                    .into_int_value();
                (ptr_ty.const_null(), bits)
            }
            // A Bool (i1) payload: zero-extend into the integer field.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 1 => {
                let ext = self
                    .builder
                    .build_int_z_extend(i, i64_ty, "bool_ext")
                    .map_err(ctx("Failed to extend Bool payload"))?;
                (ptr_ty.const_null(), ext)
            }
            // A `$` (Unit) payload — the zero `i8` — carries no bits: a zeroed slot.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 8 => {
                (ptr_ty.const_null(), i64_ty.const_zero())
            }
            // A composite payload wider than the slot — a user sum value `{i8,…}`, a nested
            // `Result`, a closure `{ptr,ptr}` — is BOXED: GC-allocate storage, copy the value
            // in, and keep the box pointer in field `.0`. `unpack_result_payload` loads it
            // back through the same pointer for the matching aggregate target type.
            BasicValueEnum::StructValue(_) => {
                let box_ptr = self.alloc_box(value.get_type())?;
                self.builder
                    .build_store(box_ptr, value)
                    .map_err(ctx("Failed to box Result payload"))?;
                (box_ptr, i64_ty.const_zero())
            }
            other => {
                return Err(format!(
                    "internal error: Result payload of type {:?} does not fit the {{ptr,i64}} slot",
                    other.get_type()
                ));
            }
        };
        let slot = self
            .builder
            .build_insert_value(slot_ty.get_undef(), ptr_field, 0, "slot_ptr")
            .map_err(ctx("Failed to pack Result ptr"))?
            .into_struct_value();
        let slot = self
            .builder
            .build_insert_value(slot, int_field, 1, "slot_int")
            .map_err(ctx("Failed to pack Result bits"))?
            .into_struct_value();
        Ok(slot.into())
    }

    /// Read a Result payload back out of the canonical `{ptr,i64}` slot as its concrete
    /// `target` type — the reverse of [`pack_result_payload`]. `target` comes from the
    /// scrutinee's oracle type (`Ok("x")` => `Text`); a still-generic/unknown payload reads
    /// as `Num` (the historical fallback), matching how generic payloads are materialized
    /// elsewhere.
    pub(super) fn unpack_result_payload(
        &self,
        slot: BasicValueEnum<'ctx>,
        target: Option<&Type>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let slot_struct = slot.into_struct_value();
        let repr = match target {
            Some(t) => self.value_repr_type(t)?,
            None => self.context.f64_type().into(),
        };
        // A `{ptr,i64}` target (Text / array) IS the whole slot.
        if repr == self.ptr_len_struct_type().into() {
            return Ok(slot);
        }
        let int_field = || -> Result<inkwell::values::IntValue<'ctx>, String> {
            self.builder
                .build_extract_value(slot_struct, 1, "slot_int")
                .map_err(ctx("Failed to read Result slot int"))
                .map(|v| v.into_int_value())
        };
        match repr {
            // A pointer target (record) rides in the pointer field.
            BasicTypeEnum::PointerType(_) => Ok(self
                .builder
                .build_extract_value(slot_struct, 0, "slot_ptr")
                .map_err(ctx("Failed to read Result slot ptr"))?),
            // A Num: reinterpret the integer field's bits back to f64.
            BasicTypeEnum::FloatType(f) => Ok(self
                .builder
                .build_bit_cast(int_field()?, f, "num_from_bits")
                .map_err(ctx("Failed to bitcast Result payload"))?),
            // A Bool: truncate the integer field back to i1.
            BasicTypeEnum::IntType(t) if t.get_bit_width() == 1 => Ok(self
                .builder
                .build_int_truncate(int_field()?, t, "bool_from_slot")
                .map_err(ctx("Failed to truncate Result payload"))?
                .into()),
            // Unit (`$`) or any other narrow int: the canonical zero `i8` Unit value.
            BasicTypeEnum::IntType(_) => Ok(self.unit_value().into()),
            // An aggregate target (a user sum value, a nested `Result`, a closure) was BOXED
            // by `pack_result_payload`: load it back through the box pointer in field `.0`.
            BasicTypeEnum::StructType(st) => {
                let box_ptr = self
                    .builder
                    .build_extract_value(slot_struct, 0, "slot_box")
                    .map_err(ctx("Failed to read Result box ptr"))?
                    .into_pointer_value();
                self.builder
                    .build_load(st, box_ptr, "unbox_payload")
                    .map_err(ctx("Failed to unbox Result payload"))
            }
            other => Err(format!(
                "internal error: Result payload target {:?} not supported",
                other
            )),
        }
    }

    /// The canonical Result LLVM struct `{ i8 tag, {ptr,i64} slot }` that `find`/`at`
    /// return — one shape for every Result (see `register_builtin_sum_types`). `elem_llvm`
    /// is unused (kept so the array methods read as intent-revealing) since the payload
    /// rides in the uniform packed slot.
    pub(super) fn result_struct_type(
        &self,
        _elem_llvm: BasicTypeEnum<'ctx>,
    ) -> inkwell::types::StructType<'ctx> {
        self.sum_struct_type("Result")
    }

    /// Build the canonical `{ i8 tag, {ptr,i64} slot }` value that `find`/`at` return, tagged
    /// as Result variant `variant` (`"Ok"` / `"NotOk"`). The tag number is read from the
    /// shared sum-variant registry (`register_builtin_sum_types`) — the same source the
    /// pattern-match consumer uses — so construction and matching can never drift apart. The
    /// `payload` is PACKED into the uniform slot (`pack_result_payload`), matching how
    /// `generate_sum_constructor` builds a Result, so both are matched/unpacked identically.
    pub(super) fn build_result(
        &mut self,
        _elem_llvm: BasicTypeEnum<'ctx>,
        variant: &str,
        payload: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let tag = self
            .sum_variants
            .get(variant)
            .map(|(t, _)| *t)
            .unwrap_or_else(|| panic!("Result variant `{variant}` is not registered"));
        let struct_ty = self.sum_struct_type("Result");
        let slot = self.pack_result_payload(payload)?;
        let tag_val = self.context.i8_type().const_int(tag as u64, false);
        let mut agg = struct_ty.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "res_tag")
            .expect("insert result tag")
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, slot, 1, "res_payload")
            .expect("insert result payload")
            .into_struct_value();
        Ok(agg.into())
    }

    /// Build a `Result` value chosen by `cond`: on the true edge, `Ok(payload)` where
    /// `payload` is produced by `ok_payload` (emitted in the `Ok` block, so it runs only
    /// when `cond` holds — the callers rely on that: an out-of-bounds index or a missing
    /// key must not load); on the false edge, `NotOk` with a zeroed slot. `label` names the
    /// blocks/alloca. Shared by array `.at` and map `.get`, whose only difference is how the
    /// `Ok` payload is computed.
    pub(super) fn build_conditional_result(
        &mut self,
        cond: inkwell::values::IntValue<'ctx>,
        elem_llvm: BasicTypeEnum<'ctx>,
        label: &str,
        ok_payload: impl FnOnce(&mut Self) -> Result<BasicValueEnum<'ctx>, String>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_ty = self.result_struct_type(elem_llvm);
        let result_ptr =
            self.create_entry_block_alloca(&format!("{label}_result"), result_ty.into())?;
        let function = self
            .current_function
            .ok_or_else(|| format!("{label} outside of function"))?;
        let ok_bb = self
            .context
            .append_basic_block(function, &format!("{label}_ok"));
        let no_bb = self
            .context
            .append_basic_block(function, &format!("{label}_no"));
        let cont_bb = self
            .context
            .append_basic_block(function, &format!("{label}_cont"));
        self.builder
            .build_conditional_branch(cond, ok_bb, no_bb)
            .map_err(ctx("Failed to branch on conditional result"))?;

        self.builder.position_at_end(ok_bb);
        let payload = ok_payload(self)?;
        let ok = self.build_result(elem_llvm, "Ok", payload)?;
        self.builder
            .build_store(result_ptr, ok)
            .map_err(ctx("Failed to store Ok"))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(ctx("Failed to branch to cont"))?;

        self.builder.position_at_end(no_bb);
        let no = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm))?;
        self.builder
            .build_store(result_ptr, no)
            .map_err(ctx("Failed to store NotOk"))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(ctx("Failed to branch to cont"))?;

        self.builder.position_at_end(cont_bb);
        self.builder
            .build_load(result_ty, result_ptr, &format!("{label}_value"))
            .map_err(ctx("Failed to load conditional result"))
    }

    /// The tagged-union LLVM struct for a sum type: `{ i8 tag, slot0, slot1, ... }`,
    /// where the slots come from the registered canonical payload layout. Both user sum
    /// types and the built-in `Result` are registered (`register_builtin_sum_types` gives
    /// Result its single `{ptr,i64}` slot); the `{ i8, double }` fallback only fires for an
    /// unregistered name, e.g. an IR-only test that skips type declaration.
    pub(super) fn sum_struct_type(&self, name: &str) -> inkwell::types::StructType<'ctx> {
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        match self.sum_layouts.get(name) {
            Some(SumLayout::PerPosition(slots)) => field_types.extend(slots.iter().copied()),
            Some(SumLayout::Union { widest, .. }) => field_types.push((*widest).into()),
            None => field_types.push(self.context.f64_type().into()),
        }
        self.context.struct_type(&field_types, false)
    }

    /// The tagged-union LLVM struct for a sum-typed *value* of type `Type::Sum`. Every sum
    /// type — user types AND the built-in `Result` (which has one canonical `{ptr,i64}` slot
    /// into which any payload is packed) — has a registered canonical layout, so this defers
    /// to [`sum_struct_type`], giving `Result` the single shape `{ i8, {ptr,i64} }` whatever
    /// its concrete `Ok`/`NotOk` payload. An UNREGISTERED `Type::Sum` (e.g. an IR-only test
    /// that skips declaration) is laid out on the fly with [`build_sum_layout`], the same
    /// rule registration would have applied.
    pub(super) fn sum_value_struct_type(
        &self,
        name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        if self.sum_layouts.contains_key(name) || variants.is_empty() {
            return Ok(self.sum_struct_type(name));
        }
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        match self.build_sum_layout(variants)? {
            SumLayout::PerPosition(slots) => field_types.extend(slots),
            SumLayout::Union { widest, .. } => field_types.push(widest.into()),
        }
        Ok(self.context.struct_type(&field_types, false))
    }

    /// A sum type's canonical payload layout: one LLVM slot per payload position, sized to
    /// the widest variant, so EVERY value of the type shares one struct shape. This is the
    /// single rule — sum registration and the by-variant lowering above both come here, and
    /// a slot they disagreed on would be a slot a payload is stored into at one size and
    /// read out of at another.
    ///
    /// Per position the slot takes the first field that is neither `$` (Unit) nor a type
    /// variable — the checker has validated that the concrete fields at a position agree —
    /// lowered through [`value_repr_type`], the shape a payload VALUE is materialized and
    /// stored as rather than the standalone lowering of its type. Falling back: a position
    /// that is only ever a type variable gets `double`, the representation a generic payload
    /// is materialized as; one that is only ever `$` (or absent) gets a zero `i8`
    /// placeholder, keeping one slot per position while storing nothing. A wholly nullary
    /// sum still gets a single `double` slot, so its `{ i8, .. }` value keeps that shape.
    pub(super) fn payload_slot_types(
        &self,
        variants: &[crate::ast::SumVariant],
    ) -> Result<Vec<BasicTypeEnum<'ctx>>, String> {
        let arity = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        let mut slots: Vec<BasicTypeEnum<'ctx>> = Vec::with_capacity(arity.max(1));
        for position in 0..arity {
            let at = || variants.iter().filter_map(|v| v.fields.get(position));
            let concrete = at().find(|f| !matches!(f, Type::Generic { .. } | Type::Unit));
            slots.push(match concrete {
                Some(field) => self.value_repr_type(field)?,
                None if at().any(|f| matches!(f, Type::Generic { .. })) => {
                    self.context.f64_type().into()
                }
                None => self.context.i8_type().into(),
            });
        }
        if slots.is_empty() {
            slots.push(self.context.f64_type().into());
        }
        Ok(slots)
    }
}
