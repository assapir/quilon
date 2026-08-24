# `core.test` — Assertions

Import with `<< core.test`. See the [Standard library index](../LANGUAGE.md#standard-library) and `examples/assert_demo.ql`.

In-language assertions for **self-verifying programs and examples**. A holding assertion does
nothing; a failing one reports to stderr and exits **101** (the Rust-panic convention), so a
broken program fails loudly in CI. Every example in `examples/` is written this way — it
asserts each result it demonstrates and exits 0 — and the examples gate runs them all under
the JIT and native AOT.

A failure reports in the standard [error frame](../LANGUAGE.md#error-messages), at **your**
call site rather than an internal hop: `assertEq` fails several calls deep inside `core.test`
and still points at the line where your program called it, including inside a helper rather
than `^`. Each assertion takes a trailing
[`site :: Site`](../LANGUAGE.md#call-site-locations--site) that the compiler fills in and the
wrappers forward.

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
[interpolation](../LANGUAGE.md#string-interpolation-and-the-render-operator-), so values appear
rendered — `Num`/`Text`/`Bool` directly, and records, sum types, and arrays through their
`` ` `` render operator. The module is pure Quilon (`corelib/test.ql`), composing the report
in-language from the `Site` fields; its only native primitives are the internal process-exit
and terminal-detection intrinsics. (See `examples/assert_demo.ql`.)
