# Corelib

The corelib — Quilon's standard library — ships with the compiler; import a module with
`<< core.<module>`. Each has its own API reference under [`docs/corelib/`](./):
signatures, behavior, and a small example per function.

| Module | Import | What it gives you |
|--------|--------|-------------------|
| [`core.io`](io.md) | `<< core.io` | Output to file descriptors and stdin: `print` / `eprint` / `write`, the `stdout` / `stderr` descriptors, and the deferred `@readStdin` line read. |
| [`core.test.report`](test/report.md) | `<< core.test.report` | The [test harness](test/report.md) `quilon test` runs and the report it prints: `describe` / `it` and `reportSuite` / `reportCase` / `reportSummary`. Pulls in `core.test`. |
| [`core.test`](test/README.md) | `<< core.test` | What a harness and reporter are built from: `failAt` for a check of your own, the run's recorded state (`casesPassed` / `casesFailed` / `nestingDepth`), and the case lifecycle (`enterSuite` / `leaveSuite` / `caseFailing` / `finishCase`). Defines no `describe` / `it` / `report*`, so [a reporter of your own](test/report.md#writing-a-reporter) can. The assertions need no import at all: `assert` / `expect` and their matchers are compiler-provided. |
| [`core.cli`](cli.md) | `<< core.cli` | Pipe-friendly helpers over the entry point's `args` / `env`: `getEnv` / `hasFlag` / `getOpt`. |
| [`core.time`](time.md) | `<< core.time` | Time primitives: the `@sleep` pause and the monotonic `now()` clock. |
| [`core.net`](net.md) | `<< core.net` | Networking: the deferred `@tcpRequest` raw TCP request exchange the HTTP client sits on. |
| [`core.http`](http.md) | `<< core.http` | An HTTP client written in Quilon over `core.net`: the `Method` sum, the `Request` / `Response` records and their methods, and the `get` shorthand. HTTP only, no TLS. |

`Text` and the operators are built-ins and need **no** import. The [concurrency model](../concurrency/README.md) that governs the `@` leaf primitives (`@readStdin`, `@sleep`) is language semantics — see that section.
