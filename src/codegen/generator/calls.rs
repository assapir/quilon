//! Call lowering: resolving what a call names (function, overload member, method, or
//! intrinsic) and emitting the call itself.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

/// The runtime intrinsic a call lowers to, as chosen by
/// [`CodeGenerator::intrinsic_lowering`].
pub(super) enum IntrinsicLowering {
    Print,
    Write,
    Now,
    ColorEnabled,
    Exit,
    /// One of the test registry's primitives, lowered by name (they share a signature).
    TestRegistry,
}

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

    /// Which runtime intrinsic a call lowers to, when it lowers to one rather than to a
    /// Quilon function. A built-in claims a call only at its own arity; an output built-in
    /// claims every such call outright (nothing may be defined at that arity), while an
    /// overload member yields to a user member of the same set that matches the argument
    /// types. This mirrors what the type checker resolved. Call lowering and the tail-call
    /// analysis both ask this one question, so they can never disagree about what a call is —
    /// a built-in name a user overloaded is an ordinary function, and a self-call in it still
    /// becomes a loop.
    pub(super) fn intrinsic_lowering(
        &self,
        name: &str,
        arguments: &[Expression],
    ) -> Option<IntrinsicLowering> {
        let lowering = match name {
            "print" | "eprint" => IntrinsicLowering::Print,
            "write" => IntrinsicLowering::Write,
            "now" => IntrinsicLowering::Now,
            "__exit" => IntrinsicLowering::Exit,
            "__color_enabled" => IntrinsicLowering::ColorEnabled,
            name if crate::ast::is_test_registry_intrinsic(name) => IntrinsicLowering::TestRegistry,
            _ => return None,
        };
        if arguments.len() != crate::ast::builtin_arity(name)? {
            return None;
        }
        // An output built-in takes any renderable value at its arity, so no user member can
        // stand between this call and it.
        if crate::ast::renderable_builtin(name).is_some() {
            return Some(lowering);
        }
        // No user member can match unless the name is an overload set here at all, and
        // inferring every argument's type is the expensive part — so ask that first.
        if !self.overloads.contains_key(name) {
            return Some(lowering);
        }
        let argument_types: Vec<Type> = arguments.iter().map(|a| self.infer_type(a)).collect();
        self.matching_overload(name, &argument_types)
            .is_none()
            .then_some(lowering)
    }

    /// The method a call resolves to on its receiver's type, as the symbol it was emitted
    /// under. Call lowering, call-site filling and the tail-call analysis all ask this one
    /// question, so none of them can disagree with the checker about which function a call
    /// names — including that only the `.` form (`member_call`) reaches a method at all.
    pub(super) fn method_symbol_for(
        &self,
        name: &str,
        arguments: &[Expression],
        member_call: bool,
    ) -> Option<String> {
        if !member_call {
            return None;
        }
        let declaring = self.declared_methods.get(name)?;
        let type_name = self.receiver_type_name(arguments.first()?)?;
        declaring
            .contains(type_name)
            .then(|| method_symbol(type_name, name))
    }

    /// Lower a call. `member_call` marks the `recv.name(args)` form, which resolves against
    /// the receiver's type alone. `span` is the CALL's own span — the location a callee
    /// whose last parameter is a `Site` receives (see [`Self::site_value`]).
    pub(super) fn generate_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        member_call: bool,
        span: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Get function name - only support direct calls for now
        let function_name = if let Expression::Identifier { name, .. } = function {
            name
        } else {
            return Err("Only direct function calls supported".to_string());
        };

        // The provided assertions, whose second argument is a matcher rather than a value —
        // lowered here, ahead of every other dispatch, since the compiler provides the form.
        // Not for a member call: `recv.assert(m)` names the receiver's `assert`, if it has
        // one, and the checker rejects it if it does not.
        if !member_call && crate::ast::is_assertion(function_name) {
            return self.generate_assertion(function_name, arguments, span);
        }

        // A leaf `@` IO primitive (`@sleep`, `@readStdin`), recognized by the `@` the parser fused
        // into the name. Handled before every other dispatch — the name is not an
        // overload/method/constructor. The `@`-identifier span carries the call's launch site.
        if let Some(primitive) = function_name.strip_prefix('@') {
            return self.generate_at_primitive(primitive, arguments, function.span());
        }

        // The `.` form resolves against the receiver's type alone, ahead of everything the
        // top-level namespace holds — the order the checker resolved the call in.
        let method_callee = self
            .method_symbol_for(function_name, arguments, member_call)
            .and_then(|symbol| self.module.get_function(&symbol));

        // Only the calls a built-in itself claims are lowered to its runtime intrinsic
        // (see `intrinsic_lowering`); anything a user member of the same set matches is
        // dispatched as an ordinary mangled call below.
        if method_callee.is_none()
            && let Some(lowering) = self.intrinsic_lowering(function_name, arguments)
        {
            return match lowering {
                // The single argument renders through its `` ` `` operator (built-in
                // default or user override). A function-typed argument is not a renderable
                // value — the type checker already rejects `print(f)` (see
                // `check_renderable_builtin_call`), so it never reaches here.
                IntrinsicLowering::Print => self.generate_print(function_name, arguments),
                IntrinsicLowering::Write => self.generate_write(arguments),
                IntrinsicLowering::Now => self.generate_now(),
                IntrinsicLowering::ColorEnabled => self.generate_color_enabled(arguments),
                // `__exit(code)` — the single native primitive `core.test` builds on
                // (terminates the process).
                IntrinsicLowering::Exit => self.generate_exit(arguments),
                IntrinsicLowering::TestRegistry => {
                    self.generate_test_registry(function_name, arguments)
                }
            };
        }

        // Built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) — RESERVED
        // on arrays. The method applies only when the receiver (`arguments[0]`) is an array;
        // the oracle confirms its element type, so this never diverts a same-named user
        // overload on a non-array receiver. Method names are lowercase and so can never
        // collide with a (Capitalized) sum-constructor name — the relative order of this
        // check and the sum-constructor block below is therefore immaterial.
        if member_call
            && crate::ast::is_array_method(function_name)
            && !arguments.is_empty()
            && matches!(
                self.oracle.expression_type(&arguments[0]),
                Some(Type::Array(_))
            )
        {
            return self.generate_array_method(function_name, arguments);
        }

        // Built-in Text methods — RESERVED on `Text`, mirroring the array-method block:
        // dispatch only when the receiver (`arguments[0]`) is a `Text` (per the oracle), so a
        // same-named user overload on another type is never diverted. Lowercase/camelCase
        // names never collide with (Capitalized) sum constructors.
        if member_call
            && crate::ast::is_text_method(function_name)
            && !arguments.is_empty()
            && matches!(self.oracle.expression_type(&arguments[0]), Some(Type::Text))
        {
            return self.generate_text_method(function_name, arguments, span);
        }

        // Built-in Map methods — RESERVED on a `Map` receiver, mirroring the array/Text
        // blocks above.
        if member_call
            && crate::ast::is_map_method(function_name)
            && !arguments.is_empty()
            && matches!(
                self.oracle.expression_type(&arguments[0]),
                Some(Type::Map(_, _))
            )
        {
            return self.generate_map_method(function_name, arguments);
        }

        // Built-in Set methods — RESERVED on a `Set` receiver.
        if member_call
            && crate::ast::is_set_method(function_name)
            && !arguments.is_empty()
            && matches!(
                self.oracle.expression_type(&arguments[0]),
                Some(Type::Set(_))
            )
        {
            return self.generate_set_method(function_name, arguments);
        }

        // Sum-type constructor with a payload (e.g. `Ok(x)`, `Circle(r)`, `Rect(w, h)`):
        // resolved from the variant registry built from the predefined Result and all
        // user `TypeDefinition::Sum` declarations. A constructor is a top-level name, so a
        // member call never reaches one.
        if !member_call
            && let Some((tag, type_name)) = self.sum_variants.get(function_name.as_str()).cloned()
        {
            return self.generate_sum_constructor(tag, &type_name, arguments);
        }

        // The receiver's method answers the call, and nothing below is consulted: a method
        // is reached through its receiver, never as a name. It can declare no `Site`
        // parameter (the checker rejects one), so its arguments are exactly those written.
        if let Some(method) = method_callee {
            let arg_values: Vec<BasicValueEnum> = arguments
                .iter()
                .map(|arg| self.generate_expression(arg))
                .collect::<Result<Vec<_>, _>>()?;
            return self.emit_call(method, &arg_values);
        }

        // Everything below looks the name up in the top-level namespace, which a member
        // call never reaches. The checker rejects a member call that resolves to nothing,
        // so reaching here means codegen and the checker disagree about the receiver's type.
        if member_call {
            return Err(format!(
                "no member '{}' on the receiver's type",
                function_name
            ));
        }

        // A local variable bound to a closure value: call it indirectly, passing the
        // captured environment as the trailing argument. Recognized by the variable's
        // recorded closure signature (see `closure_sigs`). Checked before overload
        // dispatch — a local closure binding shadows any same-named top-level function.
        if let Some((parameter_tys, ret_ty)) =
            self.closure_sigs.get(function_name.as_str()).cloned()
            && self.variables.contains_key(function_name.as_str())
        {
            return self.generate_closure_call(function_name, &parameter_tys, ret_ty, arguments);
        }

        // Overloaded function call: dispatch to the per-signature mangled symbol chosen
        // by exact argument types (the type checker has already verified a unique match).
        let overload_symbol = if self.overloads.contains_key(function_name.as_str()) {
            let arg_types: Vec<Type> = arguments.iter().map(|a| self.infer_type(a)).collect();
            self.resolve_overload_symbol(function_name, &arg_types)
        } else {
            None
        };

        // Does this call leave off a trailing `Site` for the compiler to fill in? Asked
        // before the argument values are generated, so the answer is one immutable lookup.
        let fills_call_site = self.fills_call_site(function_name, arguments, member_call);

        // The resolved callee: the overload member chosen by argument types, else the
        // plain top-level function of that name.
        let callee = match &overload_symbol {
            Some(sym) => self
                .module
                .get_function(sym)
                .ok_or_else(|| format!("Overload not found: {}", sym))?,
            None => self
                .module
                .get_function(function_name)
                .ok_or_else(|| format!("Function not found: {}", function_name))?,
        };

        // Generate argument values
        let mut arg_values: Vec<BasicValueEnum> = arguments
            .iter()
            .map(|arg| self.generate_expression(arg))
            .collect::<Result<Vec<_>, _>>()?;

        // Fill in the caller's location when the callee's last parameter is a `Site` the
        // call left off. A call that passes one explicitly (a check of your own forwarding
        // its own `site` to `failAt`) matches the full parameter list and so fills in nothing —
        // which is what propagates the USER's call site through a chain of wrappers instead
        // of reporting the innermost hop.
        if fills_call_site {
            arg_values.push(self.site_value(span)?);
        }

        self.emit_call(callee, &arg_values)
    }

    /// Whether a call to `name` passing `arguments` has its call site filled in — the callee's
    /// last parameter is a `Site` and the call left exactly that argument off.
    ///
    /// The one place codegen asks the question, so argument lowering
    /// ([`Self::generate_call`]) and tail-call detection ([`Self::is_self_tail_call`]) can
    /// never disagree about it. The rule itself is [`ast::fills_call_site`]; here it is
    /// applied to whichever signature the name resolves to — a member of an overload set,
    /// or a plain top-level function.
    pub(super) fn fills_call_site(
        &self,
        name: &str,
        arguments: &[Expression],
        member_call: bool,
    ) -> bool {
        // A method is not called by name, so it can declare no `Site` parameter (the
        // checker rejects one).
        if self
            .method_symbol_for(name, arguments, member_call)
            .is_some()
        {
            return false;
        }
        match self.overloads.contains_key(name) {
            true => {
                let arg_types: Vec<Type> = arguments.iter().map(|a| self.infer_type(a)).collect();
                self.matching_overload(name, &arg_types)
                    .is_some_and(|(parameters, _)| {
                        crate::ast::fills_call_site(parameters, arguments.len())
                    })
            }
            false => self.fn_call_site_arity.get(name) == Some(&(arguments.len() + 1)),
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

    /// Lower a leaf `@` IO primitive call to its runtime intrinsic. `site` is the span of the
    /// `@`-identifier — the call's launch site, which a fault in the launched work reports at.
    ///
    /// `@sleep(seconds :: Num) -> $` is an effect-only pause: it waits on the current fiber and
    /// yields `$`. `@readStdin() -> Text` is the first value-returning primitive: it *launches* a
    /// background stdin read and returns a DEFERRED `Text` immediately — the fiber only waits
    /// when the taint pass's force-set says a strict primitive reads the bytes.
    fn generate_at_primitive(
        &mut self,
        primitive: &str,
        arguments: &[Expression],
        site: &Span,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match primitive {
            "sleep" => {
                if arguments.len() != 1 {
                    return Err(format!(
                        "@sleep expects exactly 1 argument, got {}",
                        arguments.len()
                    ));
                }
                let BasicValueEnum::FloatValue(seconds) =
                    self.generate_expression(&arguments[0])?
                else {
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
                if !arguments.is_empty() {
                    return Err(format!(
                        "@readStdin expects no arguments, got {}",
                        arguments.len()
                    ));
                }
                let launch_site = self.site_value(site)?;
                let read = self.get_intrinsic("__read_launch")?;
                let call = self
                    .builder
                    .build_call(read, &[launch_site.into()], "read")
                    .map_err(ctx("Failed to call @readStdin"))?;
                // The result is a DEFERRED `Text` (`{ promise, -1 }`); the force-set decides
                // where it is forced. Nothing here dereferences it.
                Self::call_result_to_basic(call)
            }
            "tcpRequest" => {
                if arguments.len() != 2 {
                    return Err(format!(
                        "@tcpRequest expects exactly 2 arguments (address, requestBytes), got {}",
                        arguments.len()
                    ));
                }
                let (addr_ptr, addr_len) = self.extract_text(&arguments[0])?;
                let (req_ptr, req_len) = self.extract_text(&arguments[1])?;
                // The launch writes a DEFERRED `Result` (`Ok(responseBytes)` / `NotOk(message)`,
                // tagged deferred) into `out`; a `Result` crosses the FFI via this out-pointer, not
                // an aggregate return. The loaded value is forced at its strict-use site.
                let result_ty = self.sum_struct_type("Result");
                let out = self.create_entry_block_alloca("tcp_request_out", result_ty.into())?;
                let request = self.get_intrinsic("__tcp_request_launch")?;
                self.builder
                    .build_call(
                        request,
                        &[
                            out.into(),
                            addr_ptr.into(),
                            addr_len.into(),
                            req_ptr.into(),
                            req_len.into(),
                        ],
                        "",
                    )
                    .map_err(ctx("Failed to call @tcpRequest"))?;
                self.builder
                    .build_load(result_ty, out, "tcp_request")
                    .map_err(ctx("Failed to load @tcpRequest result"))
            }
            other => Err(format!("Unknown leaf `@` primitive `@{other}`")),
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

    /// Resolve the named record type of a method-call receiver, if known.
    ///
    /// The oracle answers first and alone when it has this expression: it is keyed by span
    /// and produced by the checker, so it knows which binding a name refers to. The
    /// per-frame `var_named_types` does not — it is flat, so an inner binding that reuses
    /// an outer record variable's name would otherwise still read as that record.
    pub(super) fn receiver_type_name<'a>(&'a self, expression: &'a Expression) -> Option<&'a str> {
        // Any record/sum-typed receiver — a variable, a sum constructor call
        // (`Rect(6, 7).area()`), a field read, a match result — dispatches by its type name.
        if let Some(ty) = self.oracle.expression_type(expression) {
            return match ty {
                Type::Named { name, .. } | Type::Sum { name, .. } => Some(name),
                _ => None,
            };
        }
        // No oracle entry (an expression the checker never saw, as the IR-only codegen
        // tests build): fall back to what emission itself recorded.
        if let Expression::Identifier { name, .. } = expression
            && let Some(type_name) = self.var_named_types.get(name)
        {
            return Some(type_name);
        }
        if let Expression::Constructor { type_name, .. } = expression {
            return Some(type_name);
        }
        None
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
