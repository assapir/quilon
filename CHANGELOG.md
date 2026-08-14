# Changelog

All notable changes to Quilon are documented here.

## Unreleased

### Added

- **Source-level debugging: `quilon build --debug` (`-g`).** Native builds can now
  emit **DWARF line-number debug info**, so a debugger (`gdb`/`lldb`) can set
  breakpoints, single-step, and print backtraces in terms of `.ql` source lines.
  Each emitted function (top-level functions, methods, closures, and the generated
  `main` wrapper) gets a `DISubprogram`, and every expression is attributed to its
  source location; the compile unit records the `.ql` file. Verify with
  `llvm-dwarfdump --debug-line ./program` / `--debug-info`. Builds are already
  unoptimized, so `--debug` only *adds* the info — the non-debug build path is
  unchanged and carries no debug info. This is **Phase 1**: line tables and
  per-function scopes only; local-variable and full-type debug info is a later
  phase. Known limitation: debug info covers the program's own source file only —
  functions imported from other modules (`<<`) carry no usable line info, because
  the debug info holds a single compile unit and source text to resolve offsets
  against; multi-file line info is a follow-up (source positions do now carry the
  identity of the file they index into, so it has what it needs). (#100)
- **Provenance watermark in native binaries.** Every executable `quilon build`
  produces now carries a plaintext watermark —
  `Built with Quilon by Assaf Sapir - github.com/assapir/quilon` — in the ELF
  `.comment` section, next to the C toolchain's own producer string. It is
  emitted as an `!llvm.ident` module metadata entry, which LLVM lowers into
  `.comment` during object generation, so it survives linking. Inspect it with
  `readelf -p .comment ./program` or `strings`. The string is a fixed
  compile-time constant (no build date), builds stay reproducible, and there is
  no runtime effect; `strip -R .comment` (or `objcopy --remove-section=.comment`)
  removes it — plain `strip --strip-all` keeps it, since GNU `strip` preserves the
  `.comment` section. `quilon run` (JIT) produces
  no artifact and so carries no watermark. (#45)
- **Runtime-library exception (licensing).** The Quilon runtime (`quilon-rt`),
  which is statically linked and embedded into every binary `quilon build`
  produces, now carries a Classpath-style linking exception on top of GPLv2 (see
  `LICENSE-EXCEPTION.md`). The exception also covers the runtime boilerplate the
  compiler emits into its output (e.g. the generated C-compatible `main()`
  wrapper). As a result, **programs you compile with Quilon are not brought under
  the GPL by that linking and may be licensed under any terms**. The compiler and
  the runtime *source* stay GPLv2 (forking either remains copyleft); the
  exception is an additional grant that frees only the combined output. libgc
  (Boehm GC), also linked in, is separately licensed under its own permissive
  terms and needs no exception. `license` fields were added to the workspace
  crates (`GPL-2.0-only`, and `GPL-2.0-only WITH Classpath-exception-2.0` for
  `quilon-rt`) to make this machine-readable. (#93)

### Removed

- **Breaking:** removed the `for n <- collection => body` loop (and its
  `for (item, index) <- …` form). Iteration is now expressed with the built-in
  array methods — `.each` for side effects, `.map`/`.filter`/`.reduce` for
  transforms/folds — and with recursion (a self-tail-call is guaranteed to lower
  to a loop, so deep recursion runs in constant stack). `for` is no longer a
  keyword; it lexes as an ordinary identifier. This also retires the `for`-body
  `:=` accumulation bug. See `examples/iteration.ql`.

### Changed

- **Breaking (layout rule):** a `(`, `[`, or `{` that is the **first token on its
  source line** no longer continues the previous expression as a call, index, or
  record constructor — it begins a **new statement**. Call arguments, index
  brackets, and constructor braces must open on the same line as the expression
  they apply to; a continuation line may still start with `.`, `|>`, or an
  operator, and an argument list (or a constructor body) opened on its
  expression's line may still span lines. Previously, with no statement
  separator, adjacent statements fused across the newline: `x = f()` followed by
  a line `(1 + 2) |> print` parsed as the call `f()(1 + 2)` (a misleading "Not a
  function" error pointing at the wrong line), `b = a` followed by
  `[3, 4].each(…)` parsed as the index `a[3, 4]`, `b = a` followed by
  `{ x = 1 }` parsed as the constructor `a { x = 1 }`, and when arities/types
  lined up the fused program compiled silently and did the wrong thing. This is
  the grammar's second line-aware rule, alongside the line-final `>` block close.
  See `examples/statements.ql`.

- **Breaking:** removed the `mut` keyword. Mutability is now the `:=` operator,
  consistent with Quilon's no-keywords, symbol-based design. `x = 0` is an
  immutable binding; `counter := 0` declares a mutable binding; `counter := counter + 1`
  reassigns it. Reassigning an immutable binding (`x := …`) is a type error, and
  immutability is now enforced by the checker.
- All `examples/*.ql` programs are now **self-asserting**: each imports `<< core.test`
  and verifies every result it demonstrates with `assert`/`assertEq`/`assertNotEq`/
  `assertOk`/`assertNotOk`, exiting 0 on success (a failing assertion aborts with exit
  101). This replaces the previous idiom of encoding a result in the process exit code
  (e.g. `factorial(5)` → exit 120). The examples gate (`tests/examples_test.rs`) is
  simplified to match: the bespoke per-example `EXPECTED_EXIT` table is gone, and the
  uniform contract is now "every runnable example exits 0" under both the JIT
  (`quilon run`) and native AOT (`quilon build`, `clang` + `gcc`), with the JIT/AOT
  parity gate kept intact.

### Fixed

- An expression in the file you compile can no longer retype an expression inside
  a module it imports. Every module is lexed on its own, so byte offsets restart
  at 0 in each one; the per-expression types codegen reads back were keyed on the
  byte range alone, so a range used in two files collided and the last one checked
  won. A program whose own code happened to sit on a corelib expression's offsets
  therefore passed `quilon check` and then failed to compile (`Function not
  found`, or a leaked LLVM verifier dump) or, worse, dispatched a library call to
  the wrong overload member and read a `Text` as a number. Source positions now
  carry the identity of the file they index into, which keeps each module's types
  its own; the offsets themselves are 32-bit, so a position stays smaller than
  before and deep nesting keeps its stack headroom.
- A self-tail-call whose argument type does not match the parameter slot it would
  be stored into now compiles to an ordinary call instead of taking the loop
  back-edge. Storing anyway wrote a wrong-sized value into the frame — silent
  corruption if the call resolution that got there ever disagreed with the
  declared parameter type. Arguments are still evaluated exactly once either way.
  A new self-asserting `examples/overload_dispatch.ql` pins down dispatch on
  argument types recovered from an array element, a match, a call, or a lambda —
  including a self-recursive overload member that must reach itself, not its
  sibling.
- Array and range literals used in a self-tail-recursive loop no longer overflow
  the stack. Two codegen paths materialized an array's `{ptr, size}` struct
  through a raw `alloca` at the current insert point — array indexing (`arr[i]`,
  to read the fields) and range lowering (`lo <- hi`, to build the result). A
  self-tail-call is lowered to a loop, so such an `alloca` re-ran every iteration
  and grew the stack without bound; a loop that built and indexed an array or
  range literal crashed at depth, violating the constant-stack guarantee. Index
  field reads now use `extractvalue` (purely in registers) and range results go
  through the shared entry-block `array_struct` helper, so no `alloca` lands in
  the loop body. (The companion escape hazard — an array literal returned from a
  function reading garbage after its frame died — was fixed earlier by
  heap-allocating literal backing stores; all sub-cases now have regression
  coverage, including a self-asserting `examples/array_literal_lifetime.ql`.)
  (#67)
- The recursive-descent parser had no recursion-depth limit, so deeply nested
  input (e.g. ~2000 nested `(`, `[`, or `{`) overflowed the native stack and
  aborted the compiler with a SIGABRT/core dump (exit 134) instead of producing
  a diagnostic — any hostile or machine-generated file could crash `quilon`.
  The parser now bounds nesting depth (max 128 levels) across every construct
  that can nest unboundedly — parenthesized expressions, array/record literals,
  block statements, `[]T` element types, nested constructor patterns
  (`Ok(Ok(…))`), chained prefix operators (`---…x`), and `:=` chains — and
  reports a clean, source-located
  `path:line:col: error: expression nesting too deep …` diagnostic (exit 1) once
  the limit is exceeded. Ordinary code nests only a handful of levels, so the
  limit affects only pathological input. (#76)
- `quilon build` was unusable from a distributed binary (a GitHub-release
  download or any machine other than the one that compiled the compiler): it
  looked for `libquilon_rt.a` at a path baked in at compile time and next to
  the binary, and releases ship the bare binary only. The runtime archive is
  now **embedded, gzip-compressed, in the `quilon` binary itself** and
  decompressed on first use into the per-user cache (`$XDG_CACHE_HOME/quilon`,
  default `~/.cache/quilon`), keyed by content hash so a new compiler never
  links a stale archive and later builds reuse the cached copy without
  re-decompressing. A system-provided archive always wins over the embedded
  one — lookup order: a `QUILON_RT_LIB` environment variable set at run time
  (developer override) → `libquilon_rt.a` next to the binary (dev loop) → the
  cached extraction → decompress the embedded copy. `quilon build` is now
  fully self-contained; the system libgc requirement remains and is tracked
  separately. (#78)
- `%` (modulo) had no codegen: it was documented and type-checked, then failed
  at `run`/`build` with an internal "Unsupported binary operation" error. It now
  lowers to the f64 remainder (LLVM `frem`, i.e. C `fmod`): works on fractional
  operands, and the result takes the dividend's sign. (#73)
- `&&` and `||` now actually short-circuit, as documented — the right operand
  is evaluated only when the left does not already decide the result. They were
  lowered as eager bitwise and/or, so `false && f(x)` ran `f`'s side effects and
  the guard idiom `i < a.size && a[i] == k` always performed the (unchecked)
  index. (#71)
- A literal or nested constructor inside a constructor pattern (`Ok(1)`,
  `Ok(Ok(x))`) was accepted by the checker but silently ignored by codegen —
  the arm matched *any* payload of the variant, so the wrong arm could win with
  no diagnostic. Such refutable sub-patterns are now a compile error (payload
  sub-patterns must be a binding or `_`); bind the payload and compare it in the
  arm body instead. (#70)
- Codegen kept per-function state (`record_types`, `var_named_types`, and — on the
  method path — `var_types`) across function emissions, so a later function reusing
  a variable name could be silently miscompiled (e.g. an array parameter `p`'s
  `p.size` mis-routed to a record-field read after an earlier function bound a
  record to `p`). Every function/method/lambda emission now starts from an empty
  per-function frame, and closures carry their captured variables' type metadata
  into the lifted frame explicitly. (#68)
- Type checking deeply nested call chains was exponential in nesting depth —
  every call site inferred its first argument twice, so ~22 nested calls took
  seconds and ~26 hung the checker outright. Since `|>` desugars to first-arg
  nesting, long pipelines hit the same wall. Each argument is now inferred
  exactly once per call site; 60-deep chains check instantly.
- Array indexing `arr[i]` is now **checked**: an out-of-bounds, negative, or NaN
  index reports a clear runtime error to stderr and exits 1. Previously it was a
  raw unchecked read — garbage values for OOB/negative indices and LLVM poison
  (undefined behavior) for NaN. A fractional in-range index still truncates
  toward zero (documented), and `.at(n)` remains the non-aborting `Ok`/`NotOk`
  form — its bounds check now also runs before the float conversion, so
  `at(0/0)` returns `NotOk` instead of branching on poison. (#74)

## 0.9.0 — "Stable basics"

The first stabilized release: a small but **verified, runnable** core of the
language. Programs run end-to-end and every example is checked in CI through both
the JIT and native-AOT paths. This is a stable *core*, **not** feature-complete —
see "Known limitations".

### Language

- **Entry point `^`** — `^ = () -> Num => …`. If the body isn't a `Num`, the
  program exits `0` (C `main`-style success), so side-effecting mains need no
  trailing `0`.
- **Modules** — `<< core.io` imports; a `>>` prefix exports a top-level item.
  Built-in `core.io` plus relative/absolute file-path imports.
- **Pipe `|>`** with first-argument injection: `x |> f` ⇒ `f(x)`,
  `x |> f(a)` ⇒ `f(x, a)`.
- **Loops** — `for n <- collection => body` (and `for (n, i) <- collection => body`).
- **`Text`** — a built-in string type (no import): `+` concatenation, `.size`
  (byte length), `.length` (grapheme-cluster count, full UTF-8).
- **`core.io`** — `print(x)` / `eprint(x)` (newline-terminated, over Num/Text/Bool),
  `write(content, fd)`, and `stdout` / `stderr`.
- Numbers (`Num`, f64), `Bool`, arrays (`.size`, indexing), records and named
  record types with methods (implicit `it`), sum types `Ok` / `NotOk` with
  pattern matching, ternary `? :`, blocks `< … >`, recursion, type inference.
- **Memory** — conservative garbage collection (Boehm GC).

### Tooling

- **`quilon run`** — compile and execute in-process via the LLVM JIT.
- **`quilon build [--linker clang|gcc] [-o out]`** — emit a native executable
  (object generated in-process via LLVM `TargetMachine`, linked against the
  bundled `libquilon_rt`).
- **`quilon compile` / `quilon check`** — emit LLVM IR / type-check only.
- **Strict CI** — deny-warnings build, blocking `clippy -D warnings`, `fmt`
  check, and a gate that runs every example through JIT **and** native AOT under
  **both** clang and gcc, asserting matching exit codes.
- Built against **LLVM 22** (inkwell).

### Known limitations (planned for a later release)

- **No generics yet** — `Text` (and other non-numeric values) inside records,
  arrays, or `Ok`/`NotOk` payloads are not supported; numeric payloads work.
- No closures, no `while` loops, no user-defined sum types, no `Unit` type.
- `Text` `.size` works only on identifier receivers in some positions.
- Boehm GC is linked dynamically (`-lgc`); self-contained static GC is planned.
- `argv` is a placeholder.
