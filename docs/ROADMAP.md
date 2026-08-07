# Quilon — Roadmap & Locked Design Decisions

This is the **authoritative plan** for Quilon's path to 1.0: the milestone roadmap, the
locked language-design decisions, and the specs for remaining work. It is the durable
record that survives across contributors and AI-agent sessions.

- **Language reference for users:** `LANGUAGE.md` (syntax + the implemented/partial/not-yet matrix).
- **Vision:** `README.md`.
- **How we build (multi-agent process + rules):** `docs/ORCHESTRATION.md`.
- **This file:** what's decided, what's done, what's next, and why.

0.9.0 is released. The work below drives toward 1.0.

---

## Status snapshot

- **M1 (diagnostics & small wins) — ✅ DONE**
- **M2 (type system) — ✅ DONE**
- **M3 (closures & functional core) — 8/9 merged;** `core.cli` paused pending prerequisites.
- **M4–M7 — not started** (design locked; see below).

Prerequisite/adjacent work in flight after M3's main items: concrete `Result` payload typing,
Text methods, array `+` concatenation, and a JIT/AOT argv-parity fix — all needed to *finish*
`core.cli` and round out the functional core.

---

## Milestone roadmap

### M1 — Diagnostics & small wins ✅
- Human-readable errors: `file:line:column` + rustc-style source context (caret/underline).
- `Unit` type `$` (type `-> $` and its sole value; `print`/`eprint` → `$`).
- VS Code extension: TypeScript + pnpm + oxlint/oxfmt; publish pipeline (`.vsix` on `vscode-v*` tags); `$` highlighting; inline diagnostics; Run/Check CodeLens above `^`; multi-char-operator highlighting fix.

### M2 — Type system ✅
- In-place field writes + setter methods on `:=` records.
- User-defined sum types with `/` separator; `Result` generalized to a normal predefined sum type.
- Explicit ad-hoc overloading (operators user-overloadable; `+`/`print` as visible overloads); `Text` comparison operators.

### M3 — Closures & functional core (8/9 merged)
- ✅ Text-in-composite codegen + the checker→codegen **type-oracle** side-table (foundation).
- ✅ Ranges: infix `<-` (`1 <- 4` inclusive; `4 <- 1` descends) → `[]Num`.
- ✅ Closures: `=` capture by value, `:=` capture by reference (GC-boxed if escaping); monomorphic.
- ✅ Guaranteed self-tail-call optimization (tail self-recursion → loop; no stack overflow).
- ✅ Array methods: `map`/`filter`/`reduce`/`each`(returns the array)/`find`→`Ok(elem)/NotOk`/`at`→`Ok(elem)/NotOk`.
- ✅ `^` entry point receives `args: []Text` and `env: [][]Text`.
- ✅ Removed the `for` loop (iterate via array methods + recursion).
- ✅ `<-` spread (prefix in literals): `[<-xs, 4]`, `{<-p, x = 9}`.
- ⏳ **`core.cli` module — PAUSED**, blocked on prerequisites (see "Remaining specs").

### M4 — Whole-program infra
- Monomorphization + defunctionalization (make function values statically visible).
- **Authoritative types in codegen:** retire codegen's lossy `infer_type` (Num fallback) and consume the type-oracle everywhere. The oracle from M3 is the start of this.

### M5 — Implicit parallelism (CPU)
- Inferred-purity parallel `map`/`filter` via `rayon` (register rayon workers with the GC).

### M6 — M:N green-thread runtime (the long pole, staged)
- 6a: single-thread fibers (`corosensei`) + reactor (`mio`) + Boehm coroutine integration (bdwgc ≥ 8.2.0; Crystal's GC+fiber recipe). 6b: cross-thread work-stealing.
- GC fork **decided**: conservative Boehm (Crystal-proven) over precise GC (which would demand compiler stack maps).

### M7 — Polish
- `quilon fmt` / linter; standard library; DWARF debug info → real VS Code debugging (unblocks a Debug CodeLens).

**Critical path:** M4 → M5 → M6. M1/M2/M3 and tooling run alongside.

---

## Locked language-design decisions

The user owns all language design. These are settled; do not relitigate without asking.

1. **Overloading is the only polymorphism (NO generics).** An overload set = multiple same-named
   top-level defs, each fully param-typed; **exact-type dispatch, no marker**. Operators are
   user-overloadable; built-in `+`/`print`/comparisons are visible overloads. **Comparison
   operators (`== != < <= > >=`) overloads MUST return `Bool`; arithmetic (`+ - * / %`) is
   unconstrained** (so `Vec*Num→Vec`, dot-product `Vec*Vec→Num` are legal). `Ok` dispatches over
   every payload incl. `Ok($)`.
2. **User sum types** use `/` as the variant separator (`Color = Red/Green/Blue`,
   `Shape = Circle(Num)/Rect(Num,Num)`). Payloads are built-in types only (Num/Text/Bool/`$`).
   `?`/`|` pattern matching, exhaustive. `Result` is a normal predefined sum type. `Ok($)` is valid
   (zero-sized, tag-only).
3. **Closures** capture by the binding operator: `=` by value (frozen), `:=` by reference
   (GC-boxed if the closure escapes; writes escape). Monomorphic (generic closures are M4).
4. **Mutability `:=`:** `=` is immutable (no field writes, no setter calls); `:=` is mutable
   (in-place `obj.x := v` and setter methods). A method is a setter iff its body writes
   `it.field := …` (inferred, no marker); setters require a `:=` receiver.
5. **Array methods as members:** `map`/`filter`/`reduce`/`each`/`find`/`at` (see M3). `.each`
   returns the array. Array methods are **non-mutating**: `map`/`filter` allocate new arrays;
   there is no in-place array-element mutation.
6. **Spread `<-`** (prefix in literals): `[<-xs, 4]` array splice; `{<-p, x = 9}` record
   functional-update (named type + methods preserved if the field set is reproduced exactly).
   Disambiguated from the infix range `<-` by position (prefix-at-element-start = spread).
7. **Concurrency = M:N green threads** (Go-style; not async/`tokio` — no function coloring). IO is
   non-blocking transparently (a green thread parks). Crate stack: `rayon` + `corosensei` + `mio` +
   a custom scheduler.
8. **Implicit parallelism = inferred purity**, no effect system, no annotations. Impure iff a
   function transitively does IO or mutates a `:=`-captured cell (both visible). Higher-order purity
   resolved by whole-program monomorphization + defunctionalization. pure → parallel, deterministic.
9. **Human-readable errors:** `file:line:column` + source context. (Shipped in M1.)
10. **Naming:** camelCase for values/functions/fields/methods; PascalCase for types/constructors;
    modules lowercase-dotted. `_` is wildcard-only. kebab-case rejected (`-` is subtraction).
    Recommendation now; `quilon fmt`/linter enforces later.
11. **`Unit` type `$`** — the type (`-> $`) and its sole value. `print`/`eprint` → `$` (but `write`
    stays `Num` = bytes). `^` with a non-`Num` body exits 0.
12. *(folded into 6/10/11)*
13. **`^` entry point may take `args: []Text` and `env: [][]Text`** — `env` is an array of
    `[key, value]` pairs (split on the first `=`). The `main()` wrapper threads `argc`/`argv`/`envp`
    in. (Shipped in M3.)
14. **Text methods** (comparison ops → Bool and `+` = concat already done). Named methods:
    - `split(sep: Text) -> []Text` — empty separator splits into graphemes; non-empty preserves
      empties (`"a,,b".split(",")` → `["a","","b"]`); empty haystack → `[""]`.
    - `trim() -> Text`.
    - `replace(from: Text, to: Text, all: Bool) -> Text` — `all=true` replaces all occurrences,
      `false` the first only; empty `from` is a no-op.
    - `contains(sub: Text) -> Bool`.
    - `indexOf(sub: Text) -> Ok(Num)/NotOk` — Result, no `-1` sentinel.
    - `slice(start: Num, end: Num) -> Text` — **grapheme** indices; out-of-range **clamps**.
    - `toUpper() -> Text`, `toLower() -> Text` — Unicode-aware.
    - **No `join` method** (kept a method surface for `split` for now; a symbol was considered and declined).
15. **`+` is the one universal concatenation/addition operator** (overloaded): `Num+Num→Num`,
    `Text+Text→Text`, and for arrays — `[]T + []T → []T` (concat), `[]T + T → []T` (append one
    element), `T + []T → []T` (prepend one element). Disambiguated by exact operand types. Edge:
    nested arrays — `[][]Num + []Num` matches *append* (RHS is one element); `[][]Num + [][]Num` is
    concat. To collapse `[]Text → Text`, compose `reduce("", (a,x)=>a+x)` — there is no `join`.
16. **String interpolation** (not built): backtick holes — `` "Hello `name`, port `port`" ``. Each
    hole's expression must be `Text` or have a `toText() -> Text` method. **`toText` is the single
    user-extensible "render to text" hook and also powers `print`** (write `print(user)`; the
    compiler routes through `toText` — never `print(toText(user))`). Built-in `toText` for
    `Num`/`Bool`; literal backtick escaped `` \` ``.
17. **Guaranteed tail-call optimization:** layer 1 (shipped) = self-tail-recursion → loop; layer 2
    (follow-up, with M4) = other/mutual tail calls via LLVM `musttail`.
18. **Result ergonomics** (not built): (a) chainable helper methods `map`/`andThen`/`orElse`/
    `getOr`/`mapErr`/`isOk`/`isNotOk` — requires **methods on sum types** (generalize the
    records-only `it`-method mechanism). (b) **propagate operator `??`** (postfix): `expr??` unwraps
    `Ok(v)` → `v`, else short-circuits, returning the `NotOk` from the enclosing function (which must
    return a `Result`).
19. **Methods return `it` by default** (chainable): a method whose body evaluates to `$`/Unit
    (setters, side-effects) returns the receiver `it`; a value-bodied method returns the value; free
    functions (`print`/`eprint`) still return `$`. `.each` returns the array (its receiver). Setters
    return the mutated `it`.
20. **Block-close is `>` at end-of-line** — a `>` followed (optional horizontal whitespace) by a
    newline or EOF is a block close; any other `>` is the greater-than operator. So `a > b` works
    everywhere; a comparison `>` just may not be the last token on a line. (Shipped in M2.)

---

## Remaining specs (work not yet merged)

### `core.cli` module (task, PAUSED — blocked on the two prerequisites below)
`corelib/cli.ql` (pure Quilon, sibling of `core.io`), library-hides-internals, pipe-friendly, skips
`argv[0]` when scanning args. Flag names accepted with or without a leading `--`.
- `getEnv(env: [][]Text, key: Text) -> Ok(Text)/NotOk(key)`.
- `hasFlag(args: []Text, flag: Text) -> Bool`.
- `getOpt(args: []Text, name: Text) -> Ok([]Text)/NotOk(name)` — recognizes `--name value` **and**
  `--name=value`; collects **all** values across repeated occurrences → `Ok([...])`; absent →
  `Ok([])`; a malformed occurrence (`--name` at end, or immediately followed by another `--flag`) →
  `NotOk(name)`.

### Prerequisites for `core.cli`
1. **Concrete `Result` payload typing** — a pattern-bound Result payload must carry its concrete
   type so it is usable at the call site (`Ok("x") ? Ok(s) | s.size`). Today it's erased to a
   generic `T` (presence-only). User-defined sum types already bind concrete payloads; mirror that
   for the built-in heterogeneous `Result`. Overlaps the M4 authoritative-types work.
2. **Text methods** (decision 14) — `split`/`slice`/`indexOf`/etc. `getOpt`'s `--name=value` split
   needs them.

### Array `+` concatenation (decision 15) — in progress
`[]T + []T → []T`, `[]T + T → []T` (append), `T + []T → []T` (prepend); new GC array, non-mutating.

### JIT/AOT argv parity (bug fix) — in progress
`quilon run f.ql a b c` must give the program `args = [f.ql, a, b, c]` (mirroring a native
`./f a b c`), not the raw `quilon run …` process argv.

### Backlog / follow-ups
- Result ergonomics (decision 18: helper methods + `??`) and methods-on-sum-types (needed for the helpers).
- String interpolation (decision 16) + `toText` hook.
- Debug CodeLens in the VS Code extension — waits for DWARF (M7).
- Binary watermark: `quilon build` embeds a plaintext "built with Quilon" marker in the native binary (low priority).
- Static/vendored Boehm GC; LLVM version bumps as inkwell supports them.
