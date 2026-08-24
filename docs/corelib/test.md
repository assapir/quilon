# `core.test` — Assertions

Import with `<< core.test`. See the [Standard library index](../LANGUAGE.md#standard-library) and `examples/assert_demo.ql`.

In-language assertions for **self-verifying programs and examples**. A holding
assertion does nothing; a **failing** one reports to **stderr** and exits the process with
code **101** (the Rust-panic convention, distinct from the 0 a passing program exits with),
so a broken program fails loudly in CI. Every example in `examples/` is written this way: it
asserts each result it demonstrates and exits 0 on success — the examples gate runs them all
under the JIT and native AOT.

A failure says **where** it failed, in the same shape as a compiler
[error](../LANGUAGE.md#error-messages) — position, message, source line, caret run:

```text
demo.ql:12:3: assertion failed: expected 42, got 41
   |
12 |   assertEq(answer(), 42)
   |   ^^^^^^^^^^^^^^^^^^^^^^
```

The location is **your** call site, never an internal hop: `assertEq` fails several calls
deep inside `core.test`, and still points at the line where your program called `assertEq`
— including when that line is inside a helper function rather than `^`. Each assertion
takes a trailing [`site :: Site`](../LANGUAGE.md#call-site-locations--site) the compiler fills in and the
wrappers forward. The report is **colored** when stderr is a terminal, and plain when it is
redirected or `NO_COLOR` / `TERM=dumb` is set — decided per run, so a CI log and a piped
build stay clean.

| Function | Effect |
|----------|--------|
| `assert(cond :: Bool) -> $` | The primitive. If `cond` is false, report `assertion failed` at the call site and exit `101`; otherwise do nothing. Returns `$` (Unit). |
| `assert(cond :: Bool, opts :: AssertOpts) -> $` | Same, but reports `opts.message` instead of the default. An [overload](../LANGUAGE.md#overloading) of `assert`. |
| `AssertOpts` | Options record for `assert`: `{ message :: Text }`. The extensible knob (more options may be added later). Records are nominal, so construct it by name: `AssertOpts { message = "..." }`. |
| `assertEq(actual, expected) -> $` | Assert `actual == expected`; the report names both (`expected 42, got 41`; `Text` values quoted, so a stray space is visible). An [overload set](../LANGUAGE.md#overloading) over `Num`/`Text`/`Bool`. |
| `assertNotEq(a, b) -> $` | Assert `a != b`; the report names the (equal) value. Overloaded over `Num`/`Text`/`Bool`. |
| `assertOk(r :: Result) -> $` | Assert `r` is `Ok`; fail on `NotOk`. |
| `assertNotOk(r :: Result) -> $` | Assert `r` is `NotOk`; fail on `Ok`. |
| `failAt(message :: Text) -> $` | Fail outright: report `message` at the caller's location and exit `101`. The primitive the assertions above are built from, and what an assertion of your own calls — take a trailing `site :: Site` and forward it, and yours reports ITS caller too. |

```quilon
<< core.test
^ = () -> $ => <
  assert(1 + 1 == 2)
  assert(1 + 1 == 2, AssertOpts { message = "math is broken" })
  assertEq(6 * 7, 42)
  assertNotEq("a", "b")
  assertOk([10, 20].at(0))       ~ Ok in bounds
  assertNotOk([10, 20].at(9))    ~ NotOk out of bounds
>
```

`assertEq`/`assertNotEq` build their message with
[interpolation](../LANGUAGE.md#string-interpolation-and-the-render-operator-), so the values appear
rendered — `Num`/`Text`/`Bool` directly, and records, sum types, and arrays through their
`` ` `` render operator. The whole module is pure Quilon (`corelib/test.ql`): the report is
composed and printed in-language from the `Site` fields, built on `assert`, `==`/`!=`,
pattern matching, and `Text.repeat` for the caret run — its only native primitives are the
internal process-exit and terminal-detection intrinsics. (See `examples/assert_demo.ql`.)
