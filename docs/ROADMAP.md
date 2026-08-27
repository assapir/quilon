# Quilon — Roadmap

The **milestone plan** for Quilon's path to 1.0: the stages and their status. High-level and
evergreen — the durable record that survives across contributors and AI-agent sessions.

- **Language semantics & the locked design decisions:** the language reference at
  [`docs/README.md`](README.md) (+ the implemented/planned feature matrix at
  [`status/feature-matrix.md`](status/feature-matrix.md)). Design decisions are documented there, not here.
- **Vision:** `README.md`.
- **How we build (multi-agent process + rules):** `docs/ORCHESTRATION.md`.
- **Specific bugs, tasks, and detailed feature specs:** GitHub issues — **not** this file.

0.9.0 is released; the stages below drive toward 1.0.

## Milestones

| Stage | Focus | Status |
|-------|-------|--------|
| **M1** | Diagnostics & small wins (readable errors, `Unit` `$`, VS Code extension) | ✅ Complete |
| **M2** | Type system (setters, user sum types `/`, ad-hoc overloading) | ✅ Complete |
| **M3** | Closures & functional core | ✅ Complete |
| **M4** | Codegen infra — authoritative types in codegen (kept); monomorphization/defunctionalization deprioritized | 🔨 In progress (partly 💤) |
| **M5** | ~~Implicit parallelism (CPU) — parallel array methods from inferred purity~~ | 💤 Deprioritized |
| **M6** | **Concurrency runtime — colorless implicit futures ([#120]) — THE core deliverable.** Stage 1: single-threaded fibers + reactor; Stage 2: M:N work-stealing + cross-thread GC ([#98]) | 🔨 In progress (Stage 1 ✅) — **core** |
| **M7** | Polish — formatter/linter, corelib, debug info | 🔨 In progress |
| **M8** | **Web — a native HTTP server built on the M6 runtime** | ⬜ Planned |

Legend: ✅ complete · 🔨 in progress · ⬜ planned · 💤 deprioritized.

**North star — _parallelism, then web_ (one spine).** Here "parallelism" means the
**colorless concurrency runtime** (M6) — *not* auto data-parallel arrays — and "web" means a
**native HTTP server built directly on that runtime** (M8). They are a single spine: the
runtime is the core deliverable, and the web server sits on top of it.

**Critical path:** M3 → **M6** (the concurrency runtime) → **M8** (web). M4's
monomorphization line and all of M5 are **deprioritized** and off the critical path — they
served an auto-data-parallelism goal the project no longer pursues.

[#120]: https://github.com/assapir/quilon/issues/120
[#98]: https://github.com/assapir/quilon/issues/98
[#60]: https://github.com/assapir/quilon/issues/60
[#49]: https://github.com/assapir/quilon/issues/49

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

### M3 — Closures & functional core ✅

| Item | Status |
|------|--------|
| Text-in-composite codegen + type-oracle side-table (foundation) | ✅ |
| Ranges — infix `<-` (`1 <- 4` inclusive; `4 <- 1` descends) | ✅ |
| Closures (`=` by value, `:=` by reference) | ✅ |
| Guaranteed self-tail-call optimization | ✅ |
| Array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) | ✅ |
| `^` entry point receives `args: []Text`, `env: [\|Text => Text\|]` | ✅ |
| Remove the `for` loop | ✅ |
| Spread — prefix `<-` in literals | ✅ |
| Text methods (`split`/`trim`/`replace`/`repeat`/`contains`/`indexOf`/`slice`/`toUpper`/`toLower`) | ✅ |
| Array concatenation via `+` | ✅ |
| Concrete `Result` payload typing | ✅ |
| `core.cli` module | ✅ |

### M4 — Codegen infra 🔨

| Item | Status |
|------|--------|
| Authoritative types in codegen (retire the lossy `infer_type`; use the type-oracle) — generally useful, **kept** | 🔨 (the type-oracle ships and is threaded through codegen; some `infer_type` callers remain) |
| Monomorphization + defunctionalization (function values statically visible) — **deprioritized**: it served the auto-data-parallelism goal (M5), now dropped | 💤 |

### M5 — Implicit parallelism (CPU) 💤 Deprioritized

**Deprioritized — off the critical path.** This milestone chased *automatic* CPU
data-parallelism over arrays (parallel `map`/`filter` inferred from purity). The project no
longer pursues auto data-parallelism: on this roadmap "parallelism" now means the M6
concurrency runtime, not parallel arrays. Kept here for history; if CPU data-parallelism
ever returns it will be **explicit** (a someday `mapParallel`), never inferred.

| Item | Status |
|------|--------|
| Inferred-purity analysis | 💤 |
| Parallel `map` / `filter` | 💤 |

### M6 — Concurrency runtime: colorless implicit futures 🔨 (core deliverable)

Quilon's north-star **"parallelism"**: the **colorless implicit-futures / promise-pipelining**
model — `@` leaf IO primitives, deferred values that propagate as they flow, and forcing
only at strict operations, so independent IO overlaps with nothing written. The design is
locked in [`concurrency/README.md`](concurrency/README.md)
and specified in full in [#120]. Built smallest-first:

| Item | Status |
|------|--------|
| **Stage 1** — single-threaded stackful fibers (`corosensei`) + IO reactor; `@` primitives (`@sleep`, `@readStdin`, `@tcpRequest`), deferred values, force-at-strict-op | ✅ |
| **Stage 2** — M:N work-stealing scheduler + Boehm GC across threads ([#98]) | ⬜ |

### M7 — Polish 🔨

| Item | Status |
|------|--------|
| `quilon fmt` / linter | ⬜ |
| `quilon test` — in-language test framework (`describe`/`it`, pluggable reporters, blocks erased from every other command), over the `assert`/`expect` matcher assertions | ✅ |
| Corelib | 🔨 (`core.io`/`core.test`/`core.test.report`/`core.cli`/`core.time`/`core.net` ship; grows with the language) |
| Debug info (→ real VS Code debugging) | ✅ (`--debug` DWARF: line tables, locals, types, multi-file — steps into corelib; entry frame reads `^`) |
| Optimization levels — `quilon build` debug vs release (O3) | ⬜ |
| Hover docs — show a function's signature/docs on hover in the editor | ⬜ |

### M8 — Web: a native HTTP server on the runtime ⬜

The **"then web"** half of the north star: a native HTTP server built directly on the M6
runtime, so many in-flight connections are cheap fibers with their IO overlapped implicitly.
Its on-ramps:

| Item | Status |
|------|--------|
| Reactor-backed input/IO — reading stdin/files/sockets, not just printing ([#60]) | 🔨 (stdin and one-shot TCP ship; files remain) |
| Statically-linked `libgc` for a self-contained server binary ([#49]) | ⬜ |
| Native HTTP server on the runtime | ⬜ |
