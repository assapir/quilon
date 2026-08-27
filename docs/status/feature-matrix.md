---
title: "Feature matrix"
sidebar:
  order: 1
---

# Feature matrix

✅ = works end-to-end with a passing run test · 🚧 = partial · ❌ = not yet

| Feature | Status |
|---|---|
| `^` entry point, native compile + JIT `run` | ✅ |
| `Num`, arithmetic, comparison, logical, ternary | ✅ |
| `Text` built-in: literals, `+`, `.size`, `.length` | ✅ |
| `Text` comparison: `==`/`!=` (equality), `<`/`<=`/`>`/`>=` (lexicographic) | ✅ |
| `Text` methods: `split`/`trim`/`trimStart`/`trimEnd`/`replaceAll`/`replace`/`repeat`/`contains`/`indexOf`/`slice`/`toUpper`/`toLower` (chainable; grapheme-based) | ✅ |
| Ad-hoc overloading: same-named typed definitions, exact-type dispatch | ✅ |
| Operator overloading as a type member (`+`, comparisons, … with `it` the left operand); built-ins as overloads | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Array methods: `map`/`filter`/`reduce`/`each`/`find`/`at` (chainable; lambda args inlined) | ✅ |
| Array `+`: concat `[]T + []T`, append `[]T + T`, prepend `T + []T` → new `[]T` (non-mutating) | ✅ |
| Maps `[\|K => V\|]`: literals, `.size`, `get` (safe, `Result`; no bracket indexing)/`has`/`set`/`remove`/`keys`/`values`/`each`; keys Num/Text/Bool or a user type; immutable | ✅ |
| Sets `[\|T\|]`: literals, `.size`, `has`/`add`/`remove`/`items`/`each`, algebra `+`/`-`/`+-` (union/difference/intersection); immutable | ✅ |
| Map/Set user-defined key types (via a `%` hash hook + `==` member) | ✅ |
| Records + field access | ✅ |
| Named record types + methods (`it`) | ✅ |
| In-place mutation of `:=` records: field writes (`obj.f := v`) + setter methods | ✅ |
| Functions, recursion, blocks, type inference | ✅ |
| Guaranteed self-tail-call optimization (tail self-recursion runs in constant stack) | ✅ |
| Closures: lexical capture (`=` by value / `:=` by reference), monomorphic | ✅ |
| Pipe `\|>` (first-argument injection) | ✅ |
| Ranges: infix `lo <- hi` → inclusive `[]Num` (descends when `lo > hi`) | ✅ |
| Spread: prefix `<-` in literals — array splice `[<-xs, 4]`, record update `{<-p, x = 9}` | ✅ |
| Pattern matching (numbers, wildcard, identifiers, sum-type variants) | ✅ |
| User-defined sum types (`/` separator), exhaustive matching, payload binding | ✅ |
| Sum-type methods: optional trailing `{ }` block (named methods, operators, render `` ` ``; `it` = the value); no fields, no `:=` methods | ✅ |
| `Result` as a normal predefined sum type (`Ok`/`NotOk`) | ✅ |
| Sum-type payloads: `Num` / `Bool` / `Text` | ✅ |
| Sum-type payload is a named **record** (`Method = Get / Post(Body)`; match binds it, reads its fields / calls its methods) | ✅ |
| Concrete `Result` payloads: a bound `Ok`/`NotOk` payload is usable at its real type (overload dispatch, across `-> Result` function **and method** boundaries) | ✅ |
| Uniform `Result` layout: a `Result` of ANY payload (`Num`/`Text`/`[]Text`/composite) passes through a generic `(r :: Result)` parameter or return — powers `isOk()`/`isNotOk()` on `getEnv`/`getOpt` | ✅ |
| Modules: `<< core.io`, `<< core.test`, `<< core.cli`, `<< core.time`, `<< core.net`, `<< core.http`, file-path imports, `>>` exports | ✅ |
| [HTTP client](../corelib/http.md): `<< core.http` — the `Method` sum and the `Request` / `Response` records with their methods (`request.send()`, `response.status()` / `.header(name)` / `.body()`), written in Quilon over `core.net`; HTTP only, no TLS | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| I/O: `@readStdin` — deferred stdin line read, forced on use | ✅ |
| Assertions: compiler-provided `assert(value, matcher)` (fatal) and `expect(value, matcher)` (recorded, test cases only), over `equals` / `contains` / `not` / `isOk` / `isNotOk`; `core.test`'s `failAt` for a check of your own | ✅ |
| Test harness: [`quilon test`](../corelib/test/README.md) over top-level `describe` / `it` blocks, which may sit in the file they test; the blocks are erased from every other command | ✅ |
| Tree-shaken imports: an item nothing in the compilation unit references is not emitted, so an import the erased `describe` blocks were the only user of reaches no build — no marker needed | ✅ |
| [Call-site locations](../functions/site.md): a trailing `site :: Site` parameter filled in by the compiler and forwarded by passing it on (track-caller) — a failing assertion reports YOUR call's `file:line:column` with a caret, identically under JIT and native | ✅ |
| Terminal-aware color: a failing assertion's report is colored on a terminal and plain when redirected or under `NO_COLOR`/`TERM=dumb`; the `\e` (ESC) string escape writes an ANSI sequence from `.qn` | ✅ |
| CLI helpers: `<< core.cli` (`getEnv` / `hasFlag` / `getOpt`; both `--name value` and `--name=value`; flag names with or without `--`) | ✅ |
| Garbage-collected memory (no manual free; self-contained binaries) | ✅ |
| `Text` (and nested arrays) in records/arrays, or as a sum-type payload (`Ok(text)`) | ✅ |
| `^` receives `args :: []Text` (argv) and `env :: [\|Text => Text\|]` (the environment as a Map) | ✅ |
| Lambdas (`x => …`) as array-method arguments (inlined per element) | ✅ |
| [Function types](../functions/README.md#function-types--higher-order-functions) (`(Num) -> Bool`, `() -> $`) + higher-order functions: a function-typed parameter called inside, taking a closure by literal or by name | ✅ |
| Generics / type variables (overloading is the only polymorphism) | ❌ |
| Overloaded or top-level function name passed as a value; a closure **returned** from a function | ❌ |
| Generic / polymorphic-capturing closures | ❌ |
| String interpolation | ❌ |
| [Colorless implicit-futures concurrency](../concurrency/README.md) — `@` leaf IO primitives, deferred values, force-at-strict-op: the fiber scheduler, the `@sleep` pause, and the value-returning `@readStdin` (deferred `Text`, forced on use) run today; cross-source overlap (networked `@get`) and the multicore runtime are still to come | 🚧 |
