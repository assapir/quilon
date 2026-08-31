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
            // i8* __alloc_array(i64 count, i64 elem_size) — GC-managed allocation of an
            // array's backing store, sized by the runtime under an overflow check.
            "__alloc_array" => ptr.fn_type(&[i64t.into(), i64t.into()], false),
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
            // void __match_fail(Site* site) — report a `?`/`|` match that no arm matched at
            // `site` (the match expression's own location) and terminate. Never returns;
            // codegen emits `unreachable` after the call.
            "__match_fail" => void.fn_type(&[ptr.into()], false),
            // i64 __range_endpoint(double value, Site* site) — one endpoint of `lo <- hi`
            // as an i64, or a report at `site` (the range expression) and status 1 for a
            // fractional, NaN, or out-of-i64 value.
            "__range_endpoint" => i64t.fn_type(&[f64t.into(), ptr.into()], false),
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
            // void __print_text_fd(i64 fd, i8* ptr, i64 len) — text + newline to fd.
            "__print_text_fd" => void.fn_type(&[i64t.into(), ptr.into(), i64t.into()], false),
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
            // ptr __envp_to_map(i8** envp) — build a `[|Text => Text|]` Map (an opaque
            // native-map pointer, the same representation `[|…|]` literals lower to) from
            // the C envp.
            "__envp_to_map" => ptr.fn_type(&[ptr.into()], false),
            // Text primitives. A `Text`/`[]Text` result is the `{ ptr, i64 }` struct; a
            // `Text` argument is passed as its (ptr, i64) fields. Only the true primitives
            // live here — the composable methods (`split`/`trim`/`contains`/`replace`/
            // `replaceAll`/`repeat`) are Quilon (`corelib/text.qn`). See `quilon-rt`.
            // { ptr, i64 } trimStart / trimEnd / toUpper / toLower / graphemes (i8*, i64);
            // `graphemes` yields a `[]Text` of the grapheme clusters.
            "__text_trim_start" | "__text_trim_end" | "__text_to_upper" | "__text_to_lower"
            | "__text_graphemes" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into()], false),
            // i64 __text_index_of(i8* hay, i64, i8* sub, i64) — grapheme index or -1.
            // i64 __text_contains(i8* hay, i64, i8* sub, i64) — 1/0; backs the `contains`
            // ASSERTION matcher (the `Text.contains` method is `core.text`'s).
            "__text_contains" | "__text_index_of" => {
                i64t.fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false)
            }
            // i64 __color_enabled(i64 fd) — 1 when `fd` is a terminal that wants color.
            "__color_enabled" => i64t.fn_type(&[i64t.into()], false),
            // { ptr, i64 } __text_slice(i8*, i64, i64 start, i64 end).
            "__text_slice" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false),
            // { ptr, i64 } __text_at(i8*, i64, i64 index) — the grapheme at `index`, or
            // the empty text for an index out of bounds (a grapheme is never empty).
            "__text_at" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), i64t.into()], false),
            // void __sleep(double seconds) — the `@sleep` leaf IO primitive: pause the
            // current fiber for `seconds` seconds, then continue.
            "__sleep" => void.fn_type(&[f64t.into()], false),
            // double __now() — read the monotonic clock, in seconds. Backs `core.time`'s
            // plain (non-`@`) `now()`; only differences between readings are meaningful.
            "__now" => f64t.fn_type(&[], false),
            // void __assert_failed(Site* site, i8* message, i64 length) — report a failing
            // `assert` at `site` and terminate with 101. Never returns; the call is left as
            // ordinary flow so an assertion composes in expression position.
            // void __expect_failed(Site*, i8*, i64) — the same report, but it marks the
            // running test case failed and RETURNS, so the suite carries on.
            "__assert_failed" | "__expect_failed" => {
                void.fn_type(&[ptr.into(), ptr.into(), i64t.into()], false)
            }
            // double __test_*() — the test registry (see `is_test_registry_intrinsic`): the
            // harness's event sink, which `core.test`'s `describe` and `it` drive. Every one
            // takes no arguments and yields a count or a depth.
            name if crate::ast::is_test_registry_intrinsic(name) => f64t.fn_type(&[], false),
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
            // and elements cross as the ABI triple `(i64 tag, i64 a, i64 b)` followed by two
            // `ptr`s: a key type's monomorphized `%` hash and `==` (both null for the
            // built-in Num/Text/Bool keys). See `quilon-rt/src/collections/` and
            // `codegen/generator/collections.rs`.
            "__map_new" | "__set_new" => ptr.fn_type(&[], false),
            // Persistent insert, returning a new table.
            "__map_set" => ptr.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                    ptr.into(),
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
                    ptr.into(),
                    ptr.into(),
                ],
                false,
            ),
            // Persistent removal, returning a new table without the key/element.
            "__map_remove" | "__set_remove" => ptr.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                    ptr.into(),
                ],
                false,
            ),
            // Membership, as 0/1.
            "__map_has" | "__set_has" => i64t.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                    ptr.into(),
                ],
                false,
            ),
            "__map_len" | "__set_len" => i64t.fn_type(&[ptr.into()], false),
            // The `a`/`b` key words of the i-th entry, in iteration order.
            "__map_key_a" | "__map_key_b" | "__set_item_a" | "__set_item_b" => {
                i64t.fn_type(&[ptr.into(), i64t.into()], false)
            }
            // Value box of the i-th entry.
            "__map_val" => ptr.fn_type(&[ptr.into(), i64t.into()], false),
            // Persistent insert, returning a new set.
            "__set_add" => ptr.fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                    i64t.into(),
                    ptr.into(),
                    ptr.into(),
                ],
                false,
            ),
            "__set_union" | "__set_diff" | "__set_intersect" => {
                ptr.fn_type(&[ptr.into(), ptr.into()], false)
            }
            other => return Err(format!("Unknown runtime intrinsic: {}", other)),
        };
        Ok(self.module.add_function(name, fn_type, None))
    }

    /// Render `expression` through its `` ` `` operator and split the resulting `Text` into
    /// the `(bytes, byte length)` pair the writing intrinsics take. The one place the output
    /// built-ins turn a value into bytes.
    fn render_text_parts(
        &mut self,
        expression: &Expression,
        label: &str,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        let rendered = self.render_expression(expression)?;
        let BasicValueEnum::StructValue(text) = rendered else {
            return Err(format!(
                "{label} expects a rendered Text, got {:?}",
                rendered.get_type()
            ));
        };
        self.split_text(text, label)
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
        let (data, len) = self.render_text_parts(&args[0], "print")?;
        let print_fn = self.get_intrinsic("__print_text_fd")?;
        self.builder
            .build_call(print_fn, &[fd_val.into(), data.into(), len.into()], "")
            .map_err(ctx("Failed to build print call"))?;
        // `print`/`eprint` yield Unit (`$`); their result is meaningless.
        Ok(self.unit_value().into())
    }

    /// Lower the `__exit(code)` primitive: convert the `Num` `code` to an `i32` and
    /// call the `__exit` runtime intrinsic, which terminates the process. This is the
    /// native primitive `core.test`'s `failAt` ends with, and what the run's summary
    /// exit code is not (that one is the entry point's return value). The intrinsic never returns, but the call is left as ordinary
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

    /// Lower one of the test registry's primitives (see
    /// [`crate::ast::is_test_registry_intrinsic`]) to its runtime intrinsic. They take no
    /// arguments and yield a `Num` — a nesting depth or a count — so the whole family
    /// lowers through this one path.
    pub(super) fn generate_test_registry(
        &mut self,
        name: &str,
        args: &[Expression],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if !args.is_empty() {
            return Err(format!("{name} expects no arguments, got {}", args.len()));
        }
        let f = self.get_intrinsic(name)?;
        let call = self
            .builder
            .build_call(f, &[], name)
            .map_err(ctx("Failed to call a test registry primitive"))?;
        Self::call_result_to_basic(call)
    }

    /// The target being emitted for. The module carries a triple only when something set one;
    /// with none set the host is the target.
    fn target_triple(&self) -> String {
        let module_triple = self.module.get_triple();
        let module_triple = module_triple.as_str().to_string_lossy().to_string();
        if module_triple.is_empty() {
            inkwell::targets::TargetMachine::get_default_triple()
                .as_str()
                .to_string_lossy()
                .to_string()
        } else {
            module_triple
        }
    }

    /// Lower a `core.info` member to the constant it names, describing the target — so a
    /// cross-compiled binary reports where it will run.
    pub(super) fn generate_info_member(
        &mut self,
        member: super::calls::InfoMember,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use super::calls::InfoMember;
        let triple = self.target_triple();
        match member {
            // A triple is `arch-vendor-os[-abi]`.
            InfoMember::Platform => {
                let arch = triple.split('-').next().unwrap_or("unknown");
                self.build_text_constant(arch)
            }
            InfoMember::Os => self.build_text_constant(os_name(&triple)),
            InfoMember::QuilonVersion => self.build_text_constant(env!("CARGO_PKG_VERSION")),
            // From LLVM, not the arch name: `powerpc64le` and `mips64el` are little-endian
            // despite their spelling, and `s390x` is 64-bit without saying so.
            InfoMember::PointerBits => {
                let bits = target_data(&triple)
                    .map(|data| u64::from(data.get_pointer_byte_size(None)) * 8)
                    .unwrap_or(u64::from(usize::BITS));
                Ok(self.context.f64_type().const_float(bits as f64).into())
            }
            InfoMember::IsBigEndian => {
                let big = match target_data(&triple) {
                    Some(data) => {
                        data.get_byte_ordering() == inkwell::targets::ByteOrdering::BigEndian
                    }
                    None => cfg!(target_endian = "big"),
                };
                Ok(self.context.bool_type().const_int(big.into(), false).into())
            }
        }
    }

    /// Lower the `now()` builtin: seconds on a monotonic clock, read through the `__now`
    /// runtime intrinsic. Only differences between two readings are meaningful.
    pub(super) fn generate_now(&mut self) -> Result<BasicValueEnum<'ctx>, String> {
        let now = self.get_intrinsic("__now")?;
        let call = self
            .builder
            .build_call(now, &[], "now")
            .map_err(ctx("Failed to call now()"))?;
        Self::call_result_to_basic(call)
    }

    /// Lower the `write(content, fd)` builtin: render `content` through its `` ` ``
    /// operator (the same render path as `print` and string interpolation — a `Text`
    /// renders as itself) and write those bytes to file descriptor `fd` (a `Num`), with no
    /// trailing newline and no substitution. Yields `Num` (bytes written).
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
        let (data, len) = self.render_text_parts(&args[0], "write")?;
        let fd_num = self.generate_expression(&args[1])?;
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

/// Triple substring to the name people use. First match wins, so a more specific needle
/// precedes a more general one.
const OS_NAMES: &[(&str, &str)] = &[
    ("darwin", "macOS"),
    ("apple", "macOS"),
    ("linux", "linux"),
    ("windows", "windows"),
    ("freebsd", "FreeBSD"),
    ("openbsd", "OpenBSD"),
    ("netbsd", "NetBSD"),
];

fn os_name(triple: &str) -> &'static str {
    OS_NAMES
        .iter()
        .find(|(needle, _)| triple.contains(needle))
        .map_or("unknown", |(_, name)| *name)
}

/// `None` when the target is not registered — the IR-only codegen tests never initialize one,
/// and fall back to the host they run on.
fn target_data(triple: &str) -> Option<inkwell::targets::TargetData> {
    use inkwell::OptimizationLevel;
    use inkwell::targets::{CodeModel, RelocMode, Target, TargetTriple};
    let triple = TargetTriple::create(triple);
    let target = Target::from_triple(&triple).ok()?;
    let machine = target.create_target_machine(
        &triple,
        "",
        "",
        OptimizationLevel::None,
        RelocMode::PIC,
        CodeModel::Default,
    )?;
    Some(machine.get_target_data())
}
