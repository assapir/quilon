# Changelog

All notable changes to Quilon are documented here.

## Unreleased

### Removed

- **Breaking:** removed the `for n <- collection => body` loop (and its
  `for (item, index) <- …` form). Iteration is now expressed with the built-in
  array methods — `.each` for side effects, `.map`/`.filter`/`.reduce` for
  transforms/folds — and with recursion (a self-tail-call is guaranteed to lower
  to a loop, so deep recursion runs in constant stack). `for` is no longer a
  keyword; it lexes as an ordinary identifier. This also retires the `for`-body
  `:=` accumulation bug. See `examples/iteration.ql`.

### Changed

- **Breaking (layout rule):** a `(` or `[` that is the **first token on its
  source line** no longer continues the previous expression as a call or index —
  it begins a **new statement**. Call arguments and index brackets must open on
  the same line as the expression they apply to; a continuation line may still
  start with `.`, `|>`, or an operator, and an argument list opened on the
  callee's line may still span lines. Previously, with no statement separator,
  adjacent statements fused across the newline: `x = f()` followed by a line
  `(1 + 2) |> print` parsed as the call `f()(1 + 2)` (a misleading "Not a
  function" error pointing at the wrong line), `b = a` followed by
  `[3, 4].each(…)` parsed as the index `a[3, 4]`, and when arities/types lined
  up the fused program compiled silently and did the wrong thing. This is the
  grammar's second line-aware rule, alongside the line-final `>` block close.
  See `examples/statements.ql`.

- **Breaking:** removed the `mut` keyword. Mutability is now the `:=` operator,
  consistent with Quilon's no-keywords, symbol-based design. `x = 0` is an
  immutable binding; `counter := 0` declares a mutable binding; `counter := counter + 1`
  reassigns it. Reassigning an immutable binding (`x := …`) is a type error, and
  immutability is now enforced by the checker.

### Fixed

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
