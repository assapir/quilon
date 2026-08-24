# `core.test` — Assertions

Import with `<< core.test`. See the [corelib index](../LANGUAGE.md#corelib) and `examples/assert_demo.qn`.

In-language assertions for **self-verifying programs and examples**. A holding assertion does
nothing; a failing one reports to stderr and exits **101** (the Rust-panic convention), so a
broken program fails loudly in CI. Every example in `examples/` is written this way — it
asserts each result it demonstrates and exits 0 — and the examples gate runs them all under
the JIT and native AOT.

A failure reports in the standard [error frame](../LANGUAGE.md#error-messages) at **your**
call site — the line where your program called the assertion, including inside a helper
rather than `^`.

| Function | Effect |
|----------|--------|
| `assert(cond :: Bool) -> $` | The primitive. If `cond` is false, report `assertion failed` at the call site and exit `101`; otherwise do nothing. Returns `$` (Unit). |
| `assert(cond :: Bool, opts :: AssertOpts) -> $` | Same, but reports `opts.message` instead of the default. An [overload](../LANGUAGE.md#overloading) of `assert`. |
| `AssertOpts` | Options record for `assert`: `{ message :: Text }`. The extensible knob (more options may be added later). Records are nominal, so construct it by name: `AssertOpts { message = "..." }`. |
| `assertEq(actual, expected) -> $` | Assert `actual == expected`; the report names both (`expected 42, got 41`; `Text` values quoted, so a stray space is visible). An [overload set](../LANGUAGE.md#overloading) over `Num`/`Text`/`Bool`. |
| `assertNotEq(a, b) -> $` | Assert `a != b`; the report names the (equal) value. Overloaded over `Num`/`Text`/`Bool`. |
| `assertOk(r :: Result) -> $` | Assert `r` is `Ok`; fail on `NotOk`. |
| `assertNotOk(r :: Result) -> $` | Assert `r` is `NotOk`; fail on `Ok`. |
| `failAt(message :: Text) -> $` | Fail outright: report `message` at the caller's location and exit `101`. Use it to build an assertion of your own that reports ITS caller — take a trailing [`site :: Site`](../LANGUAGE.md#call-site-locations--site) and forward it. |

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

`assertEq`/`assertNotEq` show their values
[rendered](../LANGUAGE.md#string-interpolation-and-the-render-operator-) — `Num`/`Text`/`Bool`
directly, and records, sum types, and arrays through their `` ` `` render operator. (See
`examples/assert_demo.qn`.)
