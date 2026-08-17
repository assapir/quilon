//! Call lowering: resolving what a call names (function, overload member, method, or
//! intrinsic) and emitting the call itself.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Emit a call to `function` with argument values that are already generated, and
    /// yield its result. The one place a direct call is built, shared by `generate_call`
    /// (which resolves the callee from a name) and the tail-call lowering.
    pub(super) fn emit_call(
        &mut self,
        function: inkwell::values::FunctionValue<'ctx>,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(ctx("Failed to build call"))?;
        Self::call_result_to_basic(call_site)
    }

    /// Lower a call. `span` is the CALL's own span — the location a callee whose last
    /// parameter is a `Site` receives (see [`Self::site_value`]).
    pub(super) fn generate_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Get function name - only support direct calls for now
        let func_name = if let Expr::Ident { name, .. } = func {
            name
        } else {
            return Err("Only direct function calls supported".to_string());
        };

        // A leaf `@` IO primitive (`@sleep`, `@readStdin`), recognized by the `@` the parser fused
        // into the name. Handled before every other dispatch — the name is not an
        // overload/method/constructor. The `@`-identifier span carries the call's launch site.
        if let Some(primitive) = func_name.strip_prefix('@') {
            return self.generate_at_primitive(primitive, args, func.span());
        }

        // `core.time`'s `now()` — a plain (non-`@`) monotonic clock read, lowered to the
        // `__now` runtime intrinsic. Its corelib body is an inert placeholder.
        if func_name == "now" && args.is_empty() {
            let now = self.get_intrinsic("__now")?;
            let call = self
                .builder
                .build_call(now, &[], "now")
                .map_err(ctx("Failed to call now()"))?;
            return Self::call_result_to_basic(call);
        }

        // Core IO builtins, lowered to runtime intrinsics (see runtime::intrinsics).
        // `print`/`eprint` are the built-in single-arg Num/Text/Bool overloads; a
        // *user* overload of the same name (a different signature) is dispatched as a
        // mangled function below, so only use the intrinsic when no user overload
        // matches the argument types.
        match func_name.as_str() {
            "print" | "eprint" => {
                // Any single argument renders through its `` ` `` operator (built-in
                // default or user override); only an EXACT user overload of `print`/`eprint`
                // (a different signature) is dispatched as a mangled call below instead. A
                // function-typed argument is not a renderable value — the type checker
                // already rejects `print(f)` (see `is_generic_print_call`), so it never
                // reaches here and this gate needs no separate `Function` exclusion.
                let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                let has_user_match = self
                    .resolve_overload_symbol(func_name, &arg_types)
                    .is_some();
                if arg_types.len() == 1 && !has_user_match {
                    return self.generate_print(func_name, args);
                }
            }
            "write" => return self.generate_write(args),
            "__color_enabled" => return self.generate_color_enabled(args),
            // `__exit(code)` — the single native primitive `core.test` builds on,
            // lowered to the `__exit` runtime intrinsic (terminates the process).
            "__exit" => return self.generate_exit(args),
            _ => {}
        }

        // Built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) — RESERVED
        // on arrays. The method applies only when the receiver (`args[0]`) is an array;
        // the oracle confirms its element type, so this never diverts a same-named user
        // overload on a non-array receiver. Method names are lowercase and so can never
        // collide with a (Capitalized) sum-constructor name — the relative order of this
        // check and the sum-constructor block below is therefore immaterial.
        if crate::ast::is_array_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Array(_)))
        {
            return self.generate_array_method(func_name, args);
        }

        // Built-in Text methods — RESERVED on `Text`, mirroring the array-method block:
        // dispatch only when the receiver (`args[0]`) is a `Text` (per the oracle), so a
        // same-named user overload on another type is never diverted. Lowercase/camelCase
        // names never collide with (Capitalized) sum constructors.
        if crate::ast::is_text_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Text))
        {
            return self.generate_text_method(func_name, args);
        }

        // Built-in Map methods — RESERVED on a `Map` receiver, mirroring the array/Text
        // blocks above.
        if crate::ast::is_map_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Map(_, _)))
        {
            return self.generate_map_method(func_name, args);
        }

        // Built-in Set methods — RESERVED on a `Set` receiver.
        if crate::ast::is_set_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Set(_)))
        {
            return self.generate_set_method(func_name, args);
        }

        // Sum-type constructor with a payload (e.g. `Ok(x)`, `Circle(r)`, `Rect(w, h)`):
        // resolved from the variant registry built from the predefined Result and all
        // user `TypeDef::Sum` declarations.
        if let Some((tag, type_name)) = self.sum_variants.get(func_name.as_str()).cloned() {
            return self.generate_sum_constructor(tag, &type_name, args);
        }

        // A local variable bound to a closure value: call it indirectly, passing the
        // captured environment as the trailing argument. Recognized by the variable's
        // recorded closure signature (see `closure_sigs`). Checked before overload
        // dispatch — a local closure binding shadows any same-named top-level function.
        if let Some((param_tys, ret_ty)) = self.closure_sigs.get(func_name.as_str()).cloned()
            && self.variables.contains_key(func_name.as_str())
        {
            return self.generate_closure_call(func_name, &param_tys, ret_ty, args);
        }

        // Overloaded function call: dispatch to the per-signature mangled symbol chosen
        // by exact argument types (the type checker has already verified a unique match).
        let overload_symbol = if self.overloads.contains_key(func_name.as_str()) {
            let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
            self.resolve_overload_symbol(func_name, &arg_types)
        } else {
            None
        };

        // Does this call leave off a trailing `Site` for the compiler to fill in? Asked
        // before the argument values are generated, so the answer is one immutable lookup.
        let fills_call_site = self.fills_call_site(func_name, args);

        // Get the function from the module. If there is no plain top-level function with this
        // name, it may be a method call: the parser desugars `recv.method(a, b)` to
        // `method(recv, a, b)`, so resolve `recv`'s named type and dispatch to `Type_method`.
        let function = if let Some(sym) = &overload_symbol {
            self.module
                .get_function(sym)
                .ok_or_else(|| format!("Overload not found: {}", sym))?
        } else {
            match self.module.get_function(func_name) {
                Some(f) => f,
                None => {
                    let mangled = args
                        .first()
                        .and_then(|recv| self.receiver_type_name(recv))
                        .map(|type_name| method_symbol(&type_name, func_name));
                    match mangled.and_then(|m| self.module.get_function(&m)) {
                        Some(f) => f,
                        None => return Err(format!("Function not found: {}", func_name)),
                    }
                }
            }
        };

        // Generate argument values
        let mut arg_values: Vec<BasicValueEnum> = args
            .iter()
            .map(|arg| self.generate_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;

        // Fill in the caller's location when the callee's last parameter is a `Site` the
        // call left off. A call that passes one explicitly (`assertEq`'s body forwarding its
        // own `site` to `assert`) matches the full parameter list and so fills in nothing —
        // which is what propagates the USER's call site through a chain of wrappers instead
        // of reporting the innermost hop.
        if fills_call_site {
            arg_values.push(self.site_value(span)?);
        }

        self.emit_call(function, &arg_values)
    }

    /// Whether a call to `name` passing `args` has its call site filled in — the callee's
    /// last parameter is a `Site` and the call left exactly that argument off.
    ///
    /// The one place codegen asks the question, so argument lowering
    /// ([`Self::generate_call`]) and tail-call detection ([`Self::is_self_tail_call`]) can
    /// never disagree about it. The rule itself is [`ast::fills_call_site`]; here it is
    /// applied to whichever signature the name resolves to — a member of an overload set,
    /// or a plain top-level function.
    pub(super) fn fills_call_site(&self, name: &str, args: &[Expr]) -> bool {
        match self.overloads.contains_key(name) {
            true => {
                let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                self.matching_overload(name, &arg_types)
                    .is_some_and(|(params, _)| crate::ast::fills_call_site(params, args.len()))
            }
            false => self.fn_call_site_arity.get(name) == Some(&(args.len() + 1)),
        }
    }

    /// The `Site` a call site fills in: a pointer to a read-only global holding the call's
    /// path, 1-based line and column, the text of its line, and how many characters of that
    /// line it spans.
    ///
    /// Every field is a compile-time constant, so the record is emitted as a `constant`
    /// global (it lands in `.rodata`) and the call passes its address. Nothing is allocated
    /// and nothing is stored at run time — which is what lets a program assert as often as
    /// it likes: a passing assertion costs its comparison and a pointer argument. Sound
    /// because a `Site` is immutable by rule: the checker refuses a write to any field of
    /// one (`TypeError::SiteIsImmutable`), which it has to, since records are handles that
    /// alias and a write through any binding would be a write to this constant.
    ///
    /// One global per distinct call site: the same span asked for twice (a tail-recursive
    /// self-call is lowered once, but the argument list is walked per tail path) reuses the
    /// first. This is compile-time interning of identical constants, not a runtime registry.
    ///
    /// A span whose file is not in the source map (a program assembled in memory, as the
    /// IR-only codegen tests do) has no location to report; the site then carries an EMPTY
    /// `file` — the documented "unknown" signal — with the position left at `1:1` so that
    /// arithmetic on it (a caret lead of `column - 1`) stays well defined for any reader.
    pub(super) fn site_value(&mut self, span: &Span) -> Result<BasicValueEnum<'ctx>, String> {
        // Keyed by the WHOLE span: the constant's `width` comes from the span's length, so
        // two spans sharing a start offset are not the same site.
        let key = (span.file, span.start, span.end);
        if let Some(existing) = self.site_globals.get(&key) {
            return Ok((*existing).into());
        }

        let location = self
            .sources
            .locate(span)
            .unwrap_or_else(crate::source_map::Location::unknown);
        let num = |value: usize| self.context.f64_type().const_float(value as f64).into();
        // Field values in the declared order, so construction cannot skew against the reads
        // (which index by `ast::site_fields`' order through the type oracle).
        let mut values: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for (name, _) in crate::ast::site_fields() {
            values.push(match name.as_str() {
                "file" => self.constant_text(&location.path),
                "line" => num(location.line),
                "column" => num(location.column),
                "excerpt" => self.constant_text(location.excerpt.as_deref().unwrap_or("")),
                "width" => num(location.width),
                other => return Err(format!("no call-site value for `Site` field `{other}`")),
            });
        }

        // The layout comes from the shared record definition — the same one a `site.line`
        // read GEPs through — rather than from the constants just built, so the two cannot
        // skew. A constant that does not fit its slot is a compiler bug, and says so here
        // instead of producing a global that reads back as garbage.
        let struct_type = self.record_struct_type(&crate::ast::site_fields())?;
        for (index, (value, slot)) in values.iter().zip(struct_type.get_field_types()).enumerate() {
            if value.get_type() != slot {
                return Err(format!(
                    "call-site `Site` field {index} is a {:?}, but its slot is a {slot:?}",
                    value.get_type()
                ));
            }
        }
        let name = format!("site.{}.{}.{}", span.file, span.start, span.end);
        let global =
            self.constant_global(struct_type, struct_type.const_named_struct(&values), &name);
        // Its natural alignment, not LLVM's PREFERRED alignment for an aggregate this size
        // (16), which would pad every site by 8 bytes — a program with a site per assertion
        // pays that per assertion.
        global.set_alignment(8);
        let pointer = global.as_pointer_value();
        self.site_globals.insert(key, pointer);
        Ok(pointer.into())
    }

    /// A `Text` `{ptr, i64}` CONSTANT for `value` — usable in a global initializer, unlike
    /// `text_literal`, which builds its value with the instruction builder.
    ///
    /// The byte constants are interned by content: a path repeats in every call site of a
    /// file, and at `OptimizationLevel::None` nothing merges duplicate globals later.
    fn constant_text(&mut self, value: &str) -> BasicValueEnum<'ctx> {
        let bytes = match self.text_constants.get(value) {
            Some(existing) => *existing,
            None => {
                let bytes = self.context.const_string(value.as_bytes(), true);
                let global = self.constant_global(bytes.get_type(), bytes, "site.str");
                global.set_alignment(1);
                let pointer = global.as_pointer_value();
                self.text_constants.insert(value.to_string(), pointer);
                pointer
            }
        };
        let len = self.context.i64_type().const_int(value.len() as u64, false);
        self.ptr_len_struct_type()
            .const_named_struct(&[bytes.into(), len.into()])
            .into()
    }

    /// Add a private, read-only global initialized to `value` — the shape every compile-time
    /// constant this file emits needs. `constant` is what makes it read-only memory (the
    /// property call-site immutability rests on) and `unnamed_addr` lets the linker merge
    /// duplicates. Callers set the alignment: LLVM's default for a global is its PREFERRED
    /// alignment, which over-pads small constants emitted in bulk.
    fn constant_global<T: inkwell::types::BasicType<'ctx>>(
        &self,
        ty: T,
        value: impl inkwell::values::BasicValue<'ctx>,
        name: &str,
    ) -> inkwell::values::GlobalValue<'ctx> {
        let global = self
            .module
            .add_global(ty, Some(AddressSpace::default()), name);
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_unnamed_addr(true);
        global
    }

    /// Lower a leaf `@` IO primitive call to its runtime intrinsic. The first is
    /// `@sleep(seconds :: Num) -> $`, an effect-only pause that waits on the current fiber
    /// and yields `$` (Unit).
    /// Lower a leaf `@` IO primitive call to its runtime intrinsic. `site` is the span of the
    /// `@`-identifier (the call's launch site), used to attach an origin to a deferred value.
    ///
    /// `@sleep(seconds :: Num) -> $` is an effect-only pause: it waits on the current fiber and
    /// yields `$`. `@readStdin() -> Text` is the first value-returning primitive: it *launches* a
    /// background stdin read and returns a DEFERRED `Text` immediately — the fiber only waits
    /// when the taint pass's force-set says a strict primitive reads the bytes.
    fn generate_at_primitive(
        &mut self,
        primitive: &str,
        args: &[Expr],
        site: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match primitive {
            "sleep" => {
                if args.len() != 1 {
                    return Err(format!(
                        "@sleep expects exactly 1 argument, got {}",
                        args.len()
                    ));
                }
                let BasicValueEnum::FloatValue(seconds) = self.generate_expr(&args[0])? else {
                    return Err("@sleep expects a Num (seconds)".to_string());
                };
                let sleep = self.get_intrinsic("__sleep")?;
                self.builder
                    .build_call(sleep, &[seconds.into()], "")
                    .map_err(ctx("Failed to call @sleep"))?;
                // `@sleep` yields `$` (Unit).
                Ok(self.unit_value().into())
            }
            "readStdin" => {
                if !args.is_empty() {
                    return Err(format!(
                        "@readStdin expects no arguments, got {}",
                        args.len()
                    ));
                }
                let (site_ptr, site_len) = self.read_launch_site(site)?;
                let read = self.get_intrinsic("__read_launch")?;
                let call = self
                    .builder
                    .build_call(read, &[site_ptr.into(), site_len.into()], "read")
                    .map_err(ctx("Failed to call @readStdin"))?;
                // The result is a DEFERRED `Text` (`{ promise, -1 }`); the force-set decides
                // where it is forced. Nothing here dereferences it.
                Self::call_result_to_basic(call)
            }
            other => Err(format!("Unknown leaf `@` primitive `@{other}`")),
        }
    }

    /// Build the `(i8* data, i64 len)` launch-site argument for `__read_launch` from the
    /// `@readStdin` call's `site` span: a `path:line:col` string constant when the driver
    /// recorded one, else a null pointer / zero length (runtime reports `<unknown>` on fault).
    fn read_launch_site(
        &mut self,
        site: &Span,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        match self.defer.read_site(site) {
            Some(description) => {
                let global = self
                    .builder
                    .build_global_string_ptr(description, "read_site")
                    .map_err(ctx("Failed to build @readStdin launch site"))?;
                let len = self
                    .context
                    .i64_type()
                    .const_int(description.len() as u64, false);
                Ok((global.as_pointer_value(), len))
            }
            None => Ok((
                self.context.ptr_type(AddressSpace::default()).const_null(),
                self.context.i64_type().const_zero(),
            )),
        }
    }

    /// Convert a call site's result to a `BasicValueEnum`, erroring if the callee returns
    /// a non-basic (e.g. void) value. Shared by the direct (`generate_call`) and indirect
    /// closure (`generate_closure_call`) call paths so both handle return kinds identically.
    pub(super) fn call_result_to_basic(
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        match call.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("call did not return a basic value".to_string()),
        }
    }

    /// Resolve the named record type of a method-call receiver, if known. Handles both a
    /// variable holding a constructed instance and a constructor expression used directly.
    pub(super) fn receiver_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident { name, .. } => self.var_named_types.get(name).cloned(),
            Expr::Constructor { type_name, .. } => Some(type_name.clone()),
            _ => None,
        }
    }

    /// Build a direct call to an already-emitted function by symbol, given the
    /// already-generated argument values. Used to lower a resolved operator/function
    /// overload to its mangled target.
    pub(super) fn build_direct_call(
        &mut self,
        symbol: &str,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .module
            .get_function(symbol)
            .ok_or_else(|| format!("Overload not found: {}", symbol))?;
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        use inkwell::values::AnyValue;
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(ctx("Failed to build call"))?;
        match call_site.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("Overloaded function does not return a basic value".to_string()),
        }
    }
}
