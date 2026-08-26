# `core.test` — Assertions and the test harness

Import with `<< core.test`. See the [corelib index](../LANGUAGE.md#corelib),
`examples/assert_demo.qn`, and `examples/test_suite.qn`.

**Assertions** (`assert`, `assertEq`, …) make a program verify itself as it runs, exiting
`101` at the first failure — what every example in `examples/` uses. The **harness**
(`describe`, `it`) groups those checks into named cases that `quilon test` runs and reports.

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

A **suite** is a `.qn` file with top-level `describe(…)` blocks and no `^` — it may declare
whatever fixtures its cases need. `quilon test` synthesizes the entry point that runs each
block in order; every other command leaves the blocks out of the program. A case checks
itself with the assertions above.

```quilon
<< core.test

describe("Text", () => <
  it("trims both ends", () => assertEq("  padded  ".trim(), "padded"))
  it("finds a part", () => assert("haystack".contains("stack")))

  describe("splitting", () => <
    it("splits on a separator", () => assertEq("a,b,c".split(",").size, 3))
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

3 cases passed
```

| Function | Effect |
|----------|--------|
| `describe(name :: Text, body :: () -> $) -> $` | A group of cases. Nestable — the report indents by depth. `body` runs immediately. |
| `it(name :: Text, body :: () -> $) -> $` | One case, reported once `body` has run. |

The **exit code** is 0 only when every case in every suite passed, so `quilon test` drops
straight into CI. A suite that fails to compile — or to parse — counts as a failed suite.

The case tree and the summary go to **stdout**; a failing assertion's
[error frame](../LANGUAGE.md#error-messages) goes to **stderr**, like every other compiler
diagnostic, so each stream reads on its own when they are captured separately.

Suites run one process each, so a failure in one does not stop the others.

### A failing case ends its suite

The assertions are fail-fast: the first failure reports and exits 101. Within a suite, that
means the failing case and everything after it go unreported, and no summary is printed — the
frame naming `file:line:column` is what identifies the failure. A suite therefore reports
"all N passed" or stops where it broke; there is no "N passed, M failed" tally across cases
yet. (A matcher API that reports every failing case is the next step here.)

### Blocks as arguments

A `< >` block closes on a [line-final `>`](../LANGUAGE.md#expressions), so a lambda
with a block body puts the call's closing `)` on the next line. Writing each `it` as a single
expression keeps that to the `describe` alone.

### Suites cost a release build nothing

`describe` is the marker — there is no `cfg` or attribute. A top-level `describe(…)` call is
test code, so `run`, `compile`, and `build` never type-check or emit it — nothing of the
harness reaches the binary. And a file with test blocks but no `^` is not a compilation unit
at all: those three pass over it in silence rather than reporting a missing entry point.
Tests can therefore sit in the module they test — beside its `>>` exports, as in
`examples/tests_alongside_code.qn`, which `examples/uses_tested_module.qn` imports. A file
that defines `^` is a program rather than a suite, so `quilon test` refuses it: blocks written
beside an `^` are stripped from the build and run nowhere.

Never type-checking it cuts both ways: **a type error inside a `describe` block is invisible
to `check`, `compile`, and `build`**, which strip the block before the checker sees it and
report success. Only `quilon test` compiles the blocks — and there a suite that fails to
compile counts as a failed suite. **Run `quilon test` in CI**, or broken test code passes
unnoticed.

### Reporters

What a run looks like is decided in `.qn`, not in the compiler. `describe` and `it` record
what happened through a reporter-agnostic registry of `__test_*` primitives — nesting depth
and a count, no rendering — and all rendering lives in three functions `core.test` exports:

| Function | Called when |
|----------|-------------|
| `reportSuite(name :: Text, depth :: Num) -> $` | A `describe` group is entered. |
| `reportCase(name :: Text, depth :: Num) -> $` | A case has run. |
| `reportSummary() -> Num` | Last, from the synthesized entry point. Prints the total and returns the exit code. |

A reporter of its own defines the same three; selecting it is a matter of pointing the
synthesized entry at another module's `reportSummary`.
