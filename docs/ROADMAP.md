# Quilon — Roadmap & Locked Design Decisions

The **authoritative plan** for Quilon's path to 1.0: the milestone stages and the locked
language-design decisions. This is the durable record that survives across contributors and
AI-agent sessions.

- **Language reference (what's implemented):** `LANGUAGE.md`.
- **Vision:** `README.md`.
- **How we build (multi-agent process + rules):** `docs/ORCHESTRATION.md`.
- **Specific bugs, tasks, and detailed feature specs:** GitHub issues — **not** this file. This roadmap
  stays high-level; per-item detail lives in the issue tracker.

0.9.0 is released; the stages below drive toward 1.0.

## Milestones

| Stage | Focus | Status |
|-------|-------|--------|
| **M1** | Diagnostics & small wins (readable errors, `Unit` `$`, VS Code extension) | ✅ Complete |
| **M2** | Type system (setters, user sum types `/`, ad-hoc overloading) | ✅ Complete |
| **M3** | Closures & functional core | 🔨 In progress |
| **M4** | Whole-program infra — monomorphization + defunctionalization; authoritative types in codegen | ⬜ Planned |
| **M5** | Implicit parallelism (CPU) — inferred purity + `rayon`-backed parallel `map`/`filter` | ⬜ Planned |
| **M6** | M:N green-thread runtime — non-blocking IO (`corosensei` + `mio` + Boehm), staged | ⬜ Planned |
| **M7** | Polish — `quilon fmt`/linter, standard library, DWARF debug info | ⬜ Planned |

Legend: ✅ complete · 🔨 in progress · ⬜ planned. **Critical path:** M4 → M5 → M6 (M6 is the long pole).

### M1 — Diagnostics & small wins ✅

| Item | Status |
|------|--------|
| Human-readable errors (`file:line:column` + source context) | ✅ |
| `Unit` type `$` | ✅ |
| VS Code extension (TypeScript, pnpm, oxlint/oxfmt, publish pipeline, `$` highlight, inline diagnostics, Run/Check CodeLens) | ✅ |

### M2 — Type system ✅

| Item | Status |
|------|--------|
| In-place field writes + setter methods on `:=` records | ✅ |
| User-defined sum types (`/` separator); `Result` generalized | ✅ |
| Explicit ad-hoc overloading (operators + `Text` comparison) | ✅ |

### M3 — Closures & functional core 🔨

| Item | Status |
|------|--------|
| Text-in-composite codegen + type-oracle side-table (foundation) | ✅ |
| Ranges — infix `<-` (`1 <- 4` inclusive; `4 <- 1` descends) | ✅ |
| Closures (`=` by value, `:=` by reference) | ✅ |
| Guaranteed self-tail-call optimization | ✅ |
| Array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) | ✅ |
| `^` entry point receives `args: []Text`, `env: [][]Text` | ✅ |
| Remove the `for` loop | ✅ |
| Spread — prefix `<-` in literals | ✅ |
| Text methods (`split`/`trim`/`replace`/`contains`/`indexOf`/`slice`/`toUpper`/`toLower`) | 🔨 |
| Array concatenation via `+` | 🔨 |
| Concrete `Result` payload typing | 🔨 |
| `core.cli` module | ⬜ Planned (blocked on Text methods + Result-payload typing) |

### M4–M7 (planned)

| Stage | Delivers |
|-------|----------|
| M4 | Monomorphization + defunctionalization (function values statically visible); retire codegen's lossy `infer_type` in favor of the type-oracle. |
| M5 | Inferred-purity analysis; parallel `map`/`filter` via `rayon` (GC-registered workers). |
| M6 | 6a single-thread fibers (`corosensei`) + reactor (`mio`) + Boehm coroutine integration; 6b cross-thread work-stealing. |
| M7 | `quilon fmt` / linter; standard library; DWARF debug info → real VS Code debugging. |

## Locked language-design decisions

The user owns all language design. These are settled — do not relitigate without asking.
"Built?" tracks whether the decision is implemented (✅) or still planned (⬜).

| # | Area | Decision | Built? |
|---|------|----------|--------|
| 1 | Polymorphism | Ad-hoc **overloading only, no generics**. Overload set = same-named fully param-typed defs; exact-type dispatch, no marker. Operators user-overloadable; `+`/`print` are visible overloads. Comparison ops (`== != < <= > >=`) must return `Bool`; arithmetic return types unconstrained. | ✅ |
| 2 | Sum types | User-defined with `/` separator; built-in payloads (Num/Text/Bool/`$`); `?`/`|` exhaustive matching; `Result` is a normal predefined sum type; `Ok($)` valid. | ✅ |
| 3 | Closures | Capture by binding operator: `=` by value (frozen), `:=` by reference (GC-boxed if escaping). Monomorphic. | ✅ |
| 4 | Mutability | `=` immutable (no field writes / setter calls); `:=` allows in-place `obj.x := v` + setter methods (setter inferred from `it.field := …`, requires a `:=` receiver). | ✅ |
| 5 | Array methods | Members, **non-mutating** (`map`/`filter` allocate new arrays; no in-place element mutation); `.each` returns the array. | ✅ |
| 6 | Spread `<-` | Prefix in literals: array splice `[<-xs, 4]` + record functional-update `{<-p, x = 9}`. Disambiguated from infix range by position. | ✅ |
| 7 | Concurrency | M:N green threads (Go-style; no `async`/coloring). Non-blocking IO transparently. Crates: `rayon` + `corosensei` + `mio`. | ⬜ |
| 8 | Implicit parallelism | Inferred purity, no effect system/annotations. Impure iff transitive IO or `:=`-cell mutation. Resolved via whole-program monomorphization. pure → parallel, deterministic. | ⬜ |
| 9 | Errors | `file:line:column` + rustc-style source context. | ✅ |
| 10 | Naming | camelCase values/fields/methods, PascalCase types/constructors, modules lowercase-dotted; `_` wildcard-only; no kebab-case. | 📏 style-guide |
| 11 | Unit `$` | The type (`-> $`) and its sole value; `print`/`eprint` → `$`; `^` with a non-`Num` body exits 0. | ✅ |
| 12 | `^` args/env | `^` may take `args: []Text` and `env: [][]Text` (env = `[key, value]` pairs). | ✅ |
| 13 | Text methods | `split(sep)→[]Text`, `trim()`, `replace(from, to, all: Bool)`, `contains(sub)→Bool`, `indexOf(sub)→Ok(Num)/NotOk`, `slice(start, end)` (grapheme indices, clamp), `toUpper`/`toLower`. **No `join`.** | 🔨 |
| 14 | `+` universal concat | `Num+Num` (add), `Text+Text` (concat), arrays: `[]T+[]T`, `[]T+T` (append), `T+[]T` (prepend). Collapse `[]Text→Text` via `reduce`, not a join. | 🔨 |
| 15 | String interpolation | Backtick holes — `` "Hello `name`" ``; each hole is `Text` or has `toText() -> Text`. **`toText` is the render hook and also powers `print`.** | ⬜ |
| 16 | Tail calls | Self-tail-recursion → loop (guaranteed). Mutual/other tail calls via LLVM `musttail` later. | ✅ (layer 1) |
| 17 | Result ergonomics | Chainable helpers (`map`/`andThen`/`orElse`/`getOr`/`mapErr`/`isOk`/`isNotOk`) via methods-on-sum-types; postfix propagate operator `??`. | ⬜ |
| 18 | Methods return `it` | A `$`/Unit-bodied method returns the receiver (chainable); value-bodied returns the value; free functions still `$`. `.each` → the array; setters → mutated `it`. | 🔨 |
| 19 | Block close | A `>` at end-of-line closes a `< … >` block; any other `>` is the greater-than operator. | ✅ |

*(`📏` = recommendation now, enforced later by `quilon fmt`.)*
