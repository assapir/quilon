# Quilon — Roadmap

The **milestone plan** for Quilon's path to 1.0: the stages and their status. High-level and
evergreen — the durable record that survives across contributors and AI-agent sessions.

- **Language semantics & the locked design decisions:** `LANGUAGE.md` (the authoritative reference +
  the implemented/planned feature matrix). Design decisions are documented there, not here.
- **Vision:** `README.md`.
- **How we build (multi-agent process + rules):** `docs/ORCHESTRATION.md`.
- **Specific bugs, tasks, and detailed feature specs:** GitHub issues — **not** this file.

0.9.0 is released; the stages below drive toward 1.0.

## Milestones

| Stage | Focus | Status |
|-------|-------|--------|
| **M1** | Diagnostics & small wins (readable errors, `Unit` `$`, VS Code extension) | ✅ Complete |
| **M2** | Type system (setters, user sum types `/`, ad-hoc overloading) | ✅ Complete |
| **M3** | Closures & functional core | 🔨 In progress |
| **M4** | Whole-program infra — monomorphization; authoritative types in codegen | ⬜ Planned |
| **M5** | Implicit parallelism (CPU) — parallel array methods from inferred purity | ⬜ Planned |
| **M6** | M:N green-thread runtime — transparent non-blocking IO | ⬜ Planned |
| **M7** | Polish — formatter/linter, standard library, debug info | ⬜ Planned |

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

### M4 — Whole-program infra ⬜

| Item | Status |
|------|--------|
| Monomorphization + defunctionalization (function values statically visible) | ⬜ |
| Authoritative types in codegen (retire the lossy `infer_type`; use the type-oracle) | ⬜ |

### M5 — Implicit parallelism (CPU) ⬜

| Item | Status |
|------|--------|
| Inferred-purity analysis | ⬜ |
| Parallel `map` / `filter` | ⬜ |

### M6 — M:N green-thread runtime ⬜

| Item | Status |
|------|--------|
| Single-threaded fibers + reactor | ⬜ |
| Coroutine ↔ GC integration | ⬜ |
| Cross-thread work-stealing | ⬜ |

### M7 — Polish ⬜

| Item | Status |
|------|--------|
| `quilon fmt` / linter | ⬜ |
| Standard library | ⬜ |
| Debug info (→ real VS Code debugging) | ⬜ |
