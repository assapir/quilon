---
title: "Quilon — Roadmap"
---

# Quilon — Roadmap

The **milestone plan** for Quilon's path to 1.0: the stages and their status. High-level and
evergreen — the durable record that survives across contributors and AI-agent sessions.

- **Language semantics & the locked design decisions:** the language reference at
  [`docs/README.md`](README.md) (+ the implemented/planned feature matrix at
  [`status/feature-matrix.md`](status/feature-matrix.md)). Design decisions are documented there, not here.
- **Vision:** `README.md`.
- **How we build (multi-agent process + rules):** `docs/ORCHESTRATION.md`.
- **Specific bugs, tasks, and detailed feature specs:** GitHub issues — **not** this file.

0.11.0 is released; the stages below drive toward 1.0.

## Milestones

| Stage | Focus | Status |
|-------|-------|--------|
| **M1** | Diagnostics & small wins (readable errors, `Unit` `$`, VS Code extension) | ✅ Complete |
| **M2** | Type system (setters, user sum types `/`, ad-hoc overloading) | ✅ Complete |
| **M3** | Closures & functional core | ✅ Complete |
| **M4** | Codegen infra — authoritative types in codegen, static methods; monomorphization/defunctionalization deprioritized | ✅ Complete (monomorphization 💤) |
| **M5** | ~~Implicit parallelism (CPU) — parallel array methods from inferred purity~~ | 💤 Deprioritized |
| **M6** | **Concurrency runtime — colorless implicit futures ([#120]) — THE core deliverable.** Stage 1: single-threaded fibers + reactor; Stage 2: M:N work-stealing + cross-thread GC | 🔨 In progress (Stage 1 ✅) — **core** |
| **M7** | Polish — formatter/linter, corelib, debug info | 🔨 In progress |
| **M8** | **Web — a native HTTP server built on the M6 runtime** | ✅ Complete |

Legend: ✅ complete · 🔨 in progress · ⬜ planned · 💤 deprioritized.

**North star — _parallelism, then web_ (one spine).** Here "parallelism" means the
**colorless concurrency runtime** (M6) — *not* auto data-parallel arrays — and "web" means a
**native HTTP server built directly on that runtime** (M8). They are a single spine: the
runtime is the core deliverable, and the web server sits on top of it.

**Critical path:** M3 → **M6** (the concurrency runtime) → **M8** (web). M4's
monomorphization line and all of M5 are **deprioritized** and off the critical path — they
served an auto-data-parallelism goal the project no longer pursues.

[#120]: https://github.com/assapir/quilon/issues/120
[#60]: https://github.com/assapir/quilon/issues/60
[#49]: https://github.com/assapir/quilon/issues/49
[#434]: https://github.com/assapir/quilon/issues/434
[#435]: https://github.com/assapir/quilon/issues/435
[#445]: https://github.com/assapir/quilon/issues/445
[#451]: https://github.com/assapir/quilon/issues/451
[#453]: https://github.com/assapir/quilon/issues/453
[#457]: https://github.com/assapir/quilon/issues/457
[#458]: https://github.com/assapir/quilon/issues/458
[#466]: https://github.com/assapir/quilon/issues/466
[#471]: https://github.com/assapir/quilon/issues/471

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

### M4 — Codegen infra ✅ (monomorphization 💤)

Remaining codegen follow-ups are tracked as GitHub issues under the `M4` label.

| Item | Status |
|------|--------|
| Authoritative types in codegen: the checker's type-oracle is the single source of truth for every type and dispatch decision; the codegen-side inference systems (`infer_type`, `closure_sigs`, `expression_is_unit`) are gone (#342) | ✅ |
| Monomorphization + defunctionalization (function values statically visible) — **deprioritized**: it served the auto-data-parallelism goal (M5), now dropped | 💤 |
| **Static methods (#259, locked):** a member whose body never reads `it` is **static** and callable on the type name (`Point.origin()`), including through a module binding (`http.Request.get(url)`) — it stays callable on a value too. A type-name call to a member that DOES read `it` is a compile error (`QN340`). See [`types/records.md#static-methods`](types/records.md#static-methods). | ✅ |

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
| **Stage 1** — single-threaded stackful fibers (`corosensei`) + IO reactor; `@` primitives (`@sleep`, `@readStdin`, `@tcpRequest`), deferred values, force-at-strict-op; a `< >` block joins every launch it made directly before returning (`allSettled`), every fault reported in launch order naming its launch site; the atomic binding syntax `@name := …`, whose reassignment's right side never waits on a deferred value; a deferred value stored in a top-level `:=` binding stays deferred there and forces on read ([#445]); hostname lookups run on the runtime's blocking-call pool ([#434]) | ✅ |
| **Stage 2** — required for 1.0: work-stealing M:N scheduler running one worker per CPU, Boehm GC across threads, atomic types (`T = @{ … }`); the fiber-sharing check has shipped ([#458]), enforced for `net.@tcpServe` and `http.@serve` handlers — the first code paths where user code runs on more than one fiber ([#120]) | ⬜ |
| Trace / explain mode | 💤 (deferred past 1.0) |

### M7 — Polish 🔨

| Item | Status |
|------|--------|
| `quilon fmt` / linter | ⬜ |
| `quilon test` — in-language test framework ([`describe`/`it`](corelib/test/README.md), the blocks erased from every other command and the harness they need tree-shaken out with them), over the `assert`/`expect` matcher assertions | ✅ |
| Corelib | 🔨 (`core.io`/`core.test`/`core.cli`/`core.time`/`core.net` ship, plus the internal `core.text` behind the built-in Text methods; grows with the language) |
| Debug info (→ real VS Code debugging) | ✅ (`--debug` DWARF: line tables, locals, types, multi-file — steps into corelib; entry frame reads `^`) |
| Optimization levels — `quilon build` debug vs release (O3) | ✅ (`build` optimizes at O3; `--debug` is unoptimized with DWARF) |
| Immutability-driven optimization — `=` methods and parameters carry LLVM `memory(read)`/`noalias` attributes; purity as a checker fact for concurrency | ⬜ |
| Hover docs — show a function's signature/docs on hover in the editor | ⬜ |

### M8 — Web: a native HTTP server on the runtime ✅

The **"then web"** half of the north star: a native HTTP server built on the M6 runtime's
Stage 1 — one worker, a fiber per connection, one reactor wait covering every connection —
so many in-flight connections are cheap fibers with their IO overlapped implicitly
([#435]). Stage 2 turns that one worker into many without changing server code. Its
on-ramps:

| Item | Status |
|------|--------|
| Reactor-backed input/IO — reading stdin/files/sockets, not just printing ([#60]) | 🔨 (stdin, one-shot TCP, and the streaming file read `@streamFile` ship; hostname resolution and regular blocking calls run on the runtime's blocking-call pool ([#434]); the whole-file `io.readFile` composition remains) |
| Statically-linked `libgc` for a self-contained server binary ([#49]) | ✅ (bdwgc built from the submodule and linked statically; a produced binary needs no libgc) |
| Fiber-sharing check — an unmarked `:=` value reachable from more than one fiber is a compile error naming `@name := …` ([#120]) | ✅ ([#458]; enforced for `net.@tcpServe` and `http.@serve` handlers, and for a signal trap's arms) |
| Native HTTP server on the runtime ([#435]) | ✅ (raw layer — `net.@tcpServe`, `Connection`, `Server.kill` ([#451]); HTTP layer — `http.@serve`, `Request.parse`, `Status` (one variant per registered code), `Response.reply` ([#457]); request bodies per Content-Length or chunked framing with a per-server cap ([#466]); keep-alive with an idle timeout ([#471])) |
| Signal trap for graceful shutdown ([#453]) | ✅ (`!>` over `core.process`'s `Signal`, one per program in the file that defines `^`; each arm its own fiber, a self-pipe/reactor dispatch installing `sigaction` only for the signals written) |
