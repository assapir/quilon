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
