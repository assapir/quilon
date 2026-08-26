# `core.test` — Assertions and the test harness

Import with `<< core.test`. See the [corelib index](../LANGUAGE.md#corelib),
`examples/assert_demo.qn`, and `examples/test_suite.qn`.

Two ways to check a program, for two purposes. **Assertions** (`assert`, `assertEq`, …) make
a program verify itself as it runs, and exit `101` at the first failure — what every example
in `examples/` uses. The **harness** (`describe`, `it`, `expect`) runs a suite of cases under
`quilon test`, reporting all of them and exiting non-zero if any failed.

## Assertions

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

## The test harness

A **suite** is a `.qn` file whose top level is `describe(…)` blocks and nothing else — no
`^`. `quilon test` synthesizes the entry point that runs each block in order; every other
command leaves the blocks out of the program.

```quilon
<< core.test

describe("Text", () => <
  it("trims both ends", () => expect("  padded  ".trim()).toBe("padded"))
  it("finds a part", () => expect("haystack").toContain("stack"))

  describe("splitting", () => <
    it("splits on a separator", () => expect("a,b,c".split(",").size).toBe(3))
  >
  )
>
)
```

```bash
quilon test                    # every suite under the current directory
quilon test tests/text.qn      # one file
quilon test tests/             # one directory
```

```
tests/text.qn
Text
  ✓ trims both ends
  ✓ finds a part
  splitting
    ✓ splits on a separator

3 cases, 3 passed, 0 failed
```

| Function | Effect |
|----------|--------|
| `describe(name :: Text, body :: () -> $) -> $` | A group of cases. Nestable — the report indents by depth. `body` runs immediately. |
| `it(name :: Text, body :: () -> $) -> $` | One case. It fails if any matcher in `body` reports; the run continues to the next case either way. |
| `expect(value) -> …Expectation` | A matcher over `value`, remembering where the `expect(…)` was written so a failure blames your call. An [overload set](../LANGUAGE.md#overloading) over `Num`/`Text`/`Bool`. |

| Matcher | On | Holds when |
|---------|----|------------|
| `.toBe(expected)` | `Num`, `Text`, `Bool` | `actual == expected` |
| `.toBeGreaterThan(limit)` / `.toBeLessThan(limit)` | `Num` | `actual > limit` / `actual < limit` |
| `.toContain(part)` | `Text` | `actual` contains `part` |
| `.toBeTruthy()` / `.toBeFalsy()` | `Bool` | `actual` is `true` / `false` |

The **exit code** is 0 only when every case in every suite passed, so `quilon test` drops
straight into CI. A suite that fails to compile counts as a failed suite.

A failing matcher renders in the same [error frame](../LANGUAGE.md#error-messages) a compiler
diagnostic uses, on **stderr**, while the case tree and the summary go to **stdout** — so
each stream reads on its own when they are captured separately:

```
tests/text.qn:6:29:
expected 5, got 4
  |
6 |   it("gets it wrong", () => expect(2 + 2).toBe(5))
  |                             ^^^^^^^^^^^^^
```

Unlike an assertion, a matcher does not exit: it **renders and continues**, which is what
lets one run report every failing case.

### Blocks as arguments

A `< >` block closes on a line-final `>`, so a lambda with a block body puts the call's
closing `)` on the next line. Writing each `it` as a single expression keeps that to the
`describe` alone.

### Suites cost a release build nothing

`describe` is the marker — there is no `cfg` or attribute. A top-level `describe(…)` call is
test code, so `check`, `compile`, `build`, and `run` never type-check or emit it, and a file
that is *only* test blocks is not a compilation unit at all: those commands pass over it
silently rather than reporting a missing `^`. Tests can therefore sit in the file they test.

### Reporters

What a run looks like is decided in `.qn`, not in the compiler. `describe`/`it`/the matchers
record what happened through a reporter-agnostic registry of `__test_*` primitives (counts
and nesting depth — the compiler renders nothing), and all rendering lives in four functions
`core.test` exports:

| Function | Called when |
|----------|-------------|
| `reportSuite(name :: Text, depth :: Num) -> $` | A `describe` group is entered. |
| `reportCase(name :: Text, depth :: Num, failed :: Num) -> $` | A case ends; `failed` is 1 or 0. |
| `reportFailure(message :: Text, file :: Text, line :: Num, column :: Num, excerpt :: Text, width :: Num) -> $` | A matcher fails. Draws the frame (via `renderFrame`, which `failAt` shares) and notes the failure. |
| `reportSummary() -> Num` | Last, from the synthesized entry point. Prints the totals and returns the exit code. |

A reporter of its own defines the same four; selecting it is a matter of pointing the
synthesized entry at another module's `reportSummary`.
