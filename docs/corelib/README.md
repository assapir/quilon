---
title: "Corelib"
sidebar:
  order: 0
---

# Corelib

The corelib — Quilon's standard library — ships with the compiler; import a module with
`<< core.<module>`. Each has its own API reference under [`docs/corelib/`](./):
signatures, behavior, and a small example per function.

| Module | Import | What it gives you |
|--------|--------|-------------------|
| [`core.io`](io.md) | `<< core.io` | Output to file descriptors and stdin: `print` / `eprint` / `write`, the `stdout` / `stderr` descriptors, and the deferred `@readStdin` line read. |
| [`core.test`](test/README.md) | `<< core.test` | The [test harness](test/README.md) `quilon test` runs and the report it prints: `test.describe` / `test.it`, plus `test.failAt` for a check of your own, the run's recorded state (`test.casesPassed` / `test.casesFailed` / `test.nestingDepth`), and the case lifecycle (`test.enterSuite` / `test.leaveSuite` / `test.caseFailing` / `test.finishCase`). The assertions need no import at all: `assert` / `expect` and their matchers are compiler-provided. |
| [`core.cli`](cli.md) | `<< core.cli` | Helpers over the entry point's `args` / `env`: `cli.getEnv` / `cli.hasFlag` / `cli.getOpt`. |
| [`core.time`](time.md) | `<< core.time` | Time primitives: the `@sleep` pause and the monotonic `now()` clock. |
| [`core.info`](info.md) | `<< core.info` | What a program can ask about itself, each closed set a sum type: `platform()`, `os()`, `pointerWidth()`, `endianness()`, `runMode()`, plus `quilonVersion()`. |
| [`core.net`](net.md) | `<< core.net` | Networking: the deferred `@tcpRequest` raw TCP request exchange the HTTP client sits on. |
| [`core.http`](http.md) | `<< core.http` | An HTTP client written in Quilon over `core.net`: the `Body` / `Method` types and the `Request` / `Response` records. HTTP only, no TLS. |

`Text` and the operators are built-ins and need **no** import. (Behind the scenes the
composable `Text` methods — `split`/`trim`/`contains`/`replace`/`replaceAll`/`repeat` —
are Quilon themselves, in the compiler-internal `core.text` module it merges in
automatically wherever one is used; only the grapheme primitives are native. See
[`Text`](../types/text.md).) The [concurrency model](../concurrency/README.md) that governs the `@` leaf primitives (`@readStdin`, `@sleep`) is language semantics — see that section.
