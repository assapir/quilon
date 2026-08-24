//! Runtime intrinsics: their LLVM declarations, and the lowering of the I/O and exit
//! builtins onto them.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

impl<'ctx> CodeGenerator<'ctx> {
    /// Declare (once) and return an external runtime intrinsic by its
    /// Quilon-internal name. These resolve to the `#[no_mangle]` symbols the runtime
    /// crate exports (or to libc, for `memcpy`) — reachable both from the in-process
    /// JIT, which maps them by address, and from an AOT link against the archive.
    ///
    /// The name must be one the runtime actually exports. Declaring anything else would
    /// emit a call that no link can resolve and no JIT mapping can fill, so the runtime's
    /// own registry gates this: a prototype here for a symbol that does not exist cannot
    /// be created in the first place. The signatures still live here, since building one
    /// needs an LLVM context the runtime crate has no business holding; a test pins them
    /// against the registry in the other direction.
    pub(super) fn get_intrinsic(&self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        // `memcpy` is libc's, not ours; everything else has to be in the registry.
        if name != "memcpy" && !quilon_rt::INTRINSICS.iter().any(|(n, _)| *n == name) {
            return Err(format!(
                "Unknown runtime intrinsic: {name} — the runtime exports no such symbol"
            ));
        }
        let ctx = self.context;
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let f64t = ctx.f64_type();
        let void = ctx.void_type();
        let fn_type = match name {
            // i8* __alloc(i64) — GC-managed allocation.
            "__alloc" => ptr.fn_type(&[i64t.into()], false),
            // void __gc_init() — initialize the Boehm GC.
            "__gc_init" => void.fn_type(&[], false),
            // void __exit(i32 code) — terminate the process with `code`. Backs the
            // `__exit(n)` primitive that `core.test`'s `assert` calls to fail. Never
            // returns (the runtime calls libc `exit`).
            "__exit" => void.fn_type(&[ctx.i32_type().into()], false),
            // void __index_fail(double index, i64 size, Site* site) — report an invalid
            // array index (out of bounds / negative / NaN) at `site` (the `arr[i]`
            // expression's own location) and terminate with status 1. Never returns;
            // codegen emits `unreachable` after the call.
            "__index_fail" => void.fn_type(&[f64t.into(), i64t.into(), ptr.into()], false),
            // i8* memcpy(i8*, i8*, i64) — libc.
            "memcpy" => ptr.fn_type(&[ptr.into(), ptr.into(), i64t.into()], false),
            // i64 __text_length(i8*, i64) — grapheme-cluster count.
            "__text_length" => i64t.fn_type(&[ptr.into(), i64t.into()], false),
            // i32 __text_cmp(i8* a, i64 alen, i8* b, i64 blen) — lexicographic byte
            // comparison, returning -1 / 0 / 1. Backs Text ==/!=/</<=/>/>=.
            "__text_cmp" => ctx
                .i32_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // i64 __write_bytes(i64 fd, i8* ptr, i64 len) — raw write, backs `write`.
            "__write_bytes" => i64t.fn_type(&[i64t.into(), ptr.into(), i64t.into()], false),
            // void __print_text_fd(i64 fd, i8*) — C string + newline to fd.
            "__print_text_fd" => void.fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __num_to_text(double) — render a Num (integer-valued without
            // decimals, else shortest round-trip). Backs the built-in `` ` `` for Num.
            "__num_to_text" => self.ptr_len_struct_type().fn_type(&[f64t.into()], false),
            // { ptr, i64 } __bool_to_text(i64) — render a Bool as "True"/"False". Backs the
            // built-in `` ` `` for Bool (capitalized, unlike the `true`/`false` literals).
            "__bool_to_text" => self.ptr_len_struct_type().fn_type(&[i64t.into()], false),
            // { ptr, i64 } __argv_to_text_array(i64 argc, i8** argv) — build a `[]Text`
            // (array of `{ptr,i64}` Text structs) from the C argc/argv. Returns the
            // `[]Text` value struct (same shape as `ptr_len_struct_type`).
            "__argv_to_text_array" => self
                .ptr_len_struct_type()
                .fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __envp_to_pairs(i8** envp) — build a `[][]Text` (array of
            // 2-element `[]Text` `[key, value]` pairs) from the C envp.
            "__envp_to_pairs" => self.ptr_len_struct_type().fn_type(&[ptr.into()], false),
            // Text methods. A `Text`/`[]Text` result is the `{ ptr, i64 }` struct; a
            // `Text` argument is passed as its (ptr, i64) fields. See `quilon-rt`.
            // { ptr, i64 } trimStart / trimEnd / toUpper / toLower (i8*, i64). `trim` is
            // composed from trimStart+trimEnd in codegen, so it has no own intrinsic.
            "__text_trim_start" | "__text_trim_end" | "__text_to_upper" | "__text_to_lower" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into()], false),
            // i64 __text_contains / __text_index_of (i8* hay, i64, i8* sub, i64).
            "__text_contains" | "__text_index_of" => {
                i64t.fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false)
            }
            // { ptr, i64 } __text_split(i8* hay, i64, i8* sep, i64) -> `[]Text`.
            "__text_split" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // i64 __color_enabled(i64 fd) — 1 when `fd` is a terminal that wants color.
            "__color_enabled" => i64t.fn_type(&[i64t.into()], false),
            // { ptr, i64 } __text_repeat(i8*, i64, double count, Site* site) — `count`
            // copies of the text. The count stays a `double` so the runtime can reject a
            // fractional or negative one rather than silently truncating it, and `site` is
            // the call's location, which such a rejection is reported at.
            "__text_repeat" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), f64t.into(), ptr.into()], false),
            // { ptr, i64 } __text_slice(i8*, i64, i64 start, i64 end).
            "__text_slice" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false),
            // { ptr, i64 } __text_replace_all(i8* hay,i64, i8* from,i64, i8* to,i64,
            // Site* site).
            "__text_replace_all" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                ],
                false,
            ),
            // { ptr, i64 } __text_replace_n(i8* hay,i64, i8* from,i64, i8* to,i64, i64 count,
            // Site* site). The trailing `Site` — as on every Text intrinsic with a fail-loud
            // contract — is the method call's own location, which the runtime frames its
            // report around.
            "__text_replace_n" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                ],
                false,
            ),
            // void __sleep(double seconds) — the `@sleep` leaf IO primitive: pause the
            // current fiber for `seconds` seconds, then continue.
            "__sleep" => void.fn_type(&[f64t.into()], false),
            // double __now() — read the monotonic clock, in seconds. Backs `core.time`'s
            // plain (non-`@`) `now()`; only differences between readings are meaningful.
            "__now" => f64t.fn_type(&[], false),
            // { ptr, i64 } __read_launch(Site* site) — the `@read` leaf IO primitive: launch
            // a background read of one line from stdin and return the DEFERRED Text
            // (`{ promise, -1 }`) immediately. `site` is the call's own location, which a
            // read fault is reported at.
            "__read_launch" => self.ptr_len_struct_type().fn_type(&[ptr.into()], false),
            // void __tcp_request_launch({i8,{ptr,i64}}* out, i8* addr,i64, i8* request,i64) — the
            // internal `@tcpRequest` leaf IO primitive: launch a background TCP request exchange
            // (connect, write the request, read until the peer closes) and write a DEFERRED
            // `Result` into `out` — `Ok(responseBytes)` on success, `NotOk(message)` on any network
            // failure. A `Result` (24 bytes) crosses the FFI via an out-pointer, not an aggregate
            // return. Backs the HTTP client; not user-facing.
            "__tcp_request_launch" => ctx.void_type().fn_type(
                &[ptr.into(), ptr.into(), i64t.into(), ptr.into(), i64t.into()],
                false,
            ),
            // { ptr, i64 } __force_text(i8* promise) — force a deferred Text: park until the
            // promise is fulfilled, then return its `{ ptr, i64 }` bytes (memoized).
            "__force_text" => self.ptr_len_struct_type().fn_type(&[ptr.into()], false),
            // void __force_result({i8,{ptr,i64}}* out, i8* promise) — force a deferred Result: park
            // until the promise is fulfilled, then write its `{ i8 tag, {ptr,i64} slot }` value
            // into `out` (memoized). An out-pointer, not an aggregate return (see above).
            "__force_result" => ctx.void_type().fn_type(&[ptr.into(), ptr.into()], false),
            // i32 __run_fiber_main(ptr entry, i32 argc, ptr argv, ptr envp) — run the
            // generated entry thunk (the C `main` signature) on a scheduler fiber, so any
            // `@` primitive it reaches has a fiber to park on. Returns the exit code.
            "__run_fiber_main" => ctx.i32_type().fn_type(
                &[ptr.into(), ctx.i32_type().into(), ptr.into(), ptr.into()],
                false,
            ),
            // Map/Set collection intrinsics. A Map/Set/value-box is an opaque `ptr`; keys
            // and elements cross as the ABI triple `(i64 tag, i64 a, i64 b)`. See
            // `quilon-rt/src/collections.rs` and `codegen/generator/collections.rs`.
            "__map_new" | "__set_new" => ptr.fn_type(&[], false),
            // Persistent insert, returning a new table.
            "__map_set" => ptr.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                ],
                false,
            ),
            // Returns the value box or null, writing 1/0 to the trailing found-out slot.
            "__map_get" => ptr.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                ],
                false,
            ),
            // Membership, as 0/1.
            "__map_has" | "__set_has" => {
                i64t.fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false)
            }
            "__map_len" | "__set_len" => i64t.fn_type(&[ptr.into()], false),
            // The `a`/`b` key words of the i-th entry, in iteration order.
            "__map_key_a" | "__map_key_b" | "__set_item_a" | "__set_item_b" => {
                i64t.fn_type(&[ptr.into(), i64t.into()], false)
            }
            // Value box of the i-th entry.
            "__map_val" => ptr.fn_type(&[ptr.into(), i64t.into()], false),
            // Persistent insert, returning a new set.
            "__set_add" => ptr.fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false),
            "__set_union" | "__set_diff" | "__set_intersect" => {
                ptr.fn_type(&[ptr.into(), ptr.into()], false)
            }
            other => return Err(format!("Unknown runtime intrinsic: {}", other)),
        };
        Ok(self.module.add_function(name, fn_type, None))
    }

    /// Lower a `print`/`eprint` builtin call: render the single argument to `Text` through
    /// its `` ` `` operator (the same render path as string interpolation), then write it —
    /// followed by a newline — to stdout (`print`, fd 1) or stderr (`eprint`, fd 2). Any
    /// value is printable because every type has a `` ` `` (built-in default or override).
    /// Yields `$` (Unit), so it composes in expression position.
    pub(super) fn generate_print(
        &mut self,
        name: &str,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "{} expects exactly 1 argument, got {}",
                name,
                args.len()
            ));
        }
        let fd = if name == "eprint" { 2 } else { 1 };
        let fd_val = self.context.i64_type().const_int(fd, false);
        let text = self.render_expression(&args[0])?;
        let data = self
            .builder
            .build_extract_value(text.into_struct_value(), 0, "print_data")
            .map_err(ctx("Failed to extract render data"))?
            .into_pointer_value();
        let print_fn = self.get_intrinsic("__print_text_fd")?;
        self.builder
            .build_call(print_fn, &[fd_val.into(), data.into()], "")
            .map_err(ctx("Failed to build print call"))?;
        // `print`/`eprint` yield Unit (`$`); their result is meaningless.
        Ok(self.unit_value().into())
    }

    /// Lower the `__exit(code)` primitive: convert the `Num` `code` to an `i32` and
    /// call the `__exit` runtime intrinsic, which terminates the process. This is the
    /// single native primitive `core.test` builds on (its `assert` calls `__exit(101)`
    /// on failure). The intrinsic never returns, but the call is left as ordinary
    /// (non-`unreachable`) flow so it composes wherever an expression is expected —
    /// e.g. a `< >` block statement or a ternary arm inside `assert` — without
    /// clashing with the surrounding construct's own terminator. The code after it is
    /// dead at runtime (the process has exited). Yields `$` (Unit).
    pub(super) fn generate_exit(
        &mut self,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "__exit expects exactly 1 argument, got {}",
                args.len()
            ));
        }
        let code = self.generate_expression(&args[0])?;
        let BasicValueEnum::FloatValue(code_f) = code else {
            return Err("__exit expects a Num exit code".to_string());
        };
        let code_i32 = self
            .builder
            .build_float_to_signed_int(code_f, self.context.i32_type(), "exit_code")
            .map_err(ctx("Failed to convert __exit code"))?;
        let exit_fn = self.get_intrinsic("__exit")?;
        self.builder
            .build_call(exit_fn, &[code_i32.into()], "")
            .map_err(ctx("Failed to build __exit call"))?;
        // `__exit` never returns; yield Unit so the call composes in expression position.
        Ok(self.unit_value().into())
    }

    /// Lower the internal `__color_enabled(fd)` primitive to its runtime intrinsic: a
    /// `Bool` saying whether `fd` is a terminal that wants ANSI styling (see the runtime for
    /// the `NO_COLOR`/`TERM`/tty rules). `core.test` uses it to decide whether to color a
    /// failure report; like `__exit` it is `__`-prefixed and exported by no module, since a
    /// raw file descriptor is not user-facing surface.
    pub(super) fn generate_color_enabled(
        &mut self,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "__color_enabled expects exactly 1 argument (fd), got {}",
                args.len()
            ));
        }
        use inkwell::values::AnyValue;
        let fd = self.text_index_arg(&args[0], "fd")?;
        let f = self.get_intrinsic("__color_enabled")?;
        let enabled = self
            .builder
            .build_call(f, &[fd.into()], "color_enabled")
            .map_err(ctx("Failed to call __color_enabled"))?
            .as_any_value_enum()
            .into_int_value();
        self.int_to_bool(enabled, "color_bool")
    }

    /// Lower the `write(content, fd)` builtin: write the raw bytes of a `Text`
    /// `content` to file descriptor `fd` (a `Num`), with no trailing newline.
    /// Yields `Num` (bytes written).
    pub(super) fn generate_write(
        &mut self,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 2 {
            return Err(format!(
                "write expects exactly 2 arguments (content, fd), got {}",
                args.len()
            ));
        }
        let content = self.generate_expression(&args[0])?;
        let fd_num = self.generate_expression(&args[1])?;
        // content must be a Text { ptr data, i64 byte_len }.
        let s = match content {
            BasicValueEnum::StructValue(s) => s,
            other => {
                return Err(format!(
                    "write expects a Text content, got {:?}",
                    other.get_type()
                ));
            }
        };
        let data = self
            .builder
            .build_extract_value(s, 0, "write_data")
            .map_err(ctx("Failed to extract text data"))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(s, 1, "write_len")
            .map_err(ctx("Failed to extract text len"))?
            .into_int_value();
        let fd_float = match fd_num {
            BasicValueEnum::FloatValue(f) => f,
            other => {
                return Err(format!(
                    "write expects a Num fd, got {:?}",
                    other.get_type()
                ));
            }
        };
        let fd_i64 = self
            .builder
            .build_float_to_signed_int(fd_float, self.context.i64_type(), "write_fd")
            .map_err(ctx("Failed to convert fd"))?;
        let write_fn = self.get_intrinsic("__write_bytes")?;
        use inkwell::values::AnyValue;
        let written = self
            .builder
            .build_call(
                write_fn,
                &[fd_i64.into(), data.into(), len.into()],
                "write_n",
            )
            .map_err(ctx("Failed to call __write_bytes"))?
            .as_any_value_enum()
            .into_int_value();
        Ok(self
            .builder
            .build_signed_int_to_float(written, self.context.f64_type(), "write_ret")
            .map_err(ctx("Failed to convert write result"))?
            .into())
    }
}
