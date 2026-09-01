---
title: "core.test — the test harness, assertions and checks"
sidebar:
  label: "core.test"
  order: 2
---

# `core.test` — the test harness, assertions and checks

**Assertions** (`assert` / `expect`) make a program verify itself as it runs — what every
example in `examples/` does. They are **compiler-provided**: no import needed,
`core.test` included. The **harness** that groups checks into named cases
(`test.describe` / `test.it`) and the report it prints come from the module — reached
through its `test` binding, like every [qualified import](../../modules/README.md) — along
with `test.failAt`, [the run's recorded state, and the case lifecycle](#what-the-run-records).

See the [corelib index](../README.md), `examples/assert_demo.qn` and
`examples/test_suite.qn`.

## Assertions

An assertion takes the **value under test first** and a **matcher second**:

```quilon ignore
assert(2 + 2, equals(4))
expect(response, isOk())
```

Two entry points, one vocabulary. They differ only in what a FAILURE does:

| Function | On failure |
|----------|-----------|
| `assert(actual, matcher) -> $` | Report at the call site and **exit 101** (the Rust-panic convention). For examples and ordinary code. |
| `expect(actual, matcher) -> $` | Report at the call site, mark the running case **failed**, and carry on. Test cases only — see [`expect` is for cases](#expect-is-for-cases). |

A holding assertion does nothing. A failure reports in the standard
[error frame](../../tooling/errors.md) at **your** call site — the line the assertion
is written on, including inside a helper rather than `^`:

```
demo.qn:4:3:
assertion failed: expected 41, got 42
  |
4 |   assert(6 * 7, equals(41))
  |   ^^^^^^^^^^^^^^^^^^^^^^^^^
```

### The matchers

| Matcher | Holds when |
|---------|-----------|
| `equals(expected)` | `actual == expected`, through the [`==` member](../../functions/overloading.md) — so `Num`/`Text`/`Bool` and any user record or sum that declares one. |
| `contains(part)` | A `Text` has `part` as a substring, or an array has an element equal to it (again through the element type's `==`). |
| `not(matcher)` | The matcher it wraps does not hold. Composes around any of them. |
| `isOk()` / `isNotOk()` | A [`Result`](../../types/sum-types.md#result-is-a-normal-sum-type) is `Ok` / `NotOk`. |

```quilon
assert(6 * 7, equals(42))
assert("assertions and matchers", contains("matcher"))
assert([2, 4, 6], not(contains(5)))
assert([10, 20].at(0), isOk())       ~ Ok in bounds
assert([10, 20].at(9), isNotOk())    ~ NotOk out of bounds
```

Both values in a report are
[rendered](../../types/text.md#string-interpolation-and-the-render-operator-) — `Num`/`Text`/`Bool`
directly, records, sum types and arrays through their `` ` `` operator, and a `Text` is
quoted, so a trailing space or an empty string is visible. A matcher applied to a type it
cannot read — `equals` on a type with no `==` member, `contains` on a `Num`, `isOk` on a sum
with no such variant — is a compile error naming what is missing.

The matchers are compiler-provided, not written in `.qn`: a matcher holds a value of the type
under test, which without generics would need one matcher type per type. You can still
compose the provided ones; a genuinely new matcher kind waits for generics. Until then,
[`failAt`](#building-a-check-of-your-own) builds a check of your own.

### Building a check of your own

| Function | Effect |
|----------|--------|
| `test.failAt(message :: Text) -> $` | Report `message` at the caller's location and exit `101` — the same frame `assert` uses. Take a trailing [`site :: Site`](../../functions/site.md) and forward it, and the report blames ITS caller. |

```quilon
<< core.test

assertEven = (n :: Num, site :: Site) -> $ => <
  n % 2 == 0 ? $ : test.failAt("`n` is odd", site)
>
```

## Suites, groups and cases

A **suite** is any `.qn` file with top-level `test.describe(…)` blocks — a file of nothing but
tests, or the module or program they test ([below](#tests-beside-the-code-costing-a-release-build-nothing)),
with whatever fixtures the cases need. `quilon test` synthesizes the entry point that runs
each block in order; every other command leaves the blocks out of the program. A case checks
itself with `expect`.

```quilon
<< core.test

test.describe("Text", () => <
  test.it("trims both ends", () => expect("  padded  ".trim(), equals("padded")))
  test.it("finds a part", () => expect("haystack", contains("stack")))

  test.describe("splitting", () => <
    test.it("splits on a separator", () => expect("a,b,c".split(",").size, equals(3)))
  >)
>)
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

3 passed, 0 failed
```

| Function | Effect |
|----------|--------|
| `test.describe(name :: Text, body :: () -> $) -> $` | A group of cases. Nestable — the report indents by depth. `body` runs immediately. |
| `test.it(name :: Text, body :: () -> $) -> $` | One case, reported once `body` has run, `✓` or `✗`. |

The compiler recognizes a top-level `test.describe(…)` call **by name** — there is no attribute
or `cfg`. What the report looks like is currently fixed.

The **exit code** is 0 only when every case in every suite passed, so `quilon test` drops
straight into CI. A suite that fails to compile — or to parse — counts as a failed suite.

The case tree and the summary go to **stdout**; a failing assertion's
[error frame](../../tooling/errors.md) goes to **stderr**, like every other compiler
diagnostic, so each stream reads on its own when they are captured separately.

Suites run one process each, so a failure in one does not stop the others. A suite that
imports no harness at all is a compile error at its first `test.describe`, naming the import that
fixes it — never a silent run with no output.

## A failing case does not stop the run

The first failing `expect` in a case **skips the rest of that case** — the assertions after it
do not run, and their subjects are never evaluated — and the suite carries on with the next
case. Every case is therefore reported, the way it went, and the summary is a real tally:

```
arithmetic
  ✓ holds
  ✗ does not hold
  ✓ runs after the failure

2 passed, 1 failed
```

`assert` inside a case is still fatal, and ends the run where it failed. Use it for a
precondition a case cannot meaningfully continue past.

## `expect` is for cases

`expect` marks the running **case** failed, and `test.it` is what closes a case and tallies
it — so an `expect` belongs inside a `test.it`, inside a `test.describe`. Anywhere else it
is a **compile error** pointing at `assert`:

- outside a `test.describe` block there is no run to record with, the blocks being stripped
  from `run`, `compile`, and `build`;
- inside a `test.describe` but outside a `test.it` there is no case to mark, so the failure would be
  printed and never counted.

The rule is lexical, so a top-level helper a case calls uses `assert`, not `expect`.

## Tests beside the code, costing a release build nothing

Tests may sit in the same file as the code they test — beside its `>>` exports, beside its `^`,
or both, as in `examples/tests_alongside_code.qn`. `describe` is the marker; there is no `cfg`
or attribute:

- `check`, `compile`, `build`, `run`: every top-level `test.describe(…)` is **erased** before the
  checker sees it, so no test code of yours is checked or emitted. A file whose blocks are all
  it has is no program at all — `compile`, `build`, and `run` pass over it in silence rather
  than reporting a missing entry point.
- `quilon test`: the blocks are **compiled and run**, under the entry point it synthesizes. A
  file's own `^` is not the test run's, so it is ignored rather than called.

The `<< core.test` the blocks need takes no marker of its own. Erasing them leaves nothing in
the file naming `describe` or `it`, and a function nothing reaches is not emitted, so the
harness is shaken out of the build along with the blocks it served. The shaking is over
EMISSION, not scope: an imported module is still resolved and type-checked, and the names it
exports still occupy the importer's scope, so a program cannot define a `describe` of its own
beside `<< core.test`.

Never type-checking them cuts both ways: **a type error inside a `describe` block is invisible
to `check`, `compile`, `build`, and `run`** — they erase the block before the checker sees it
and succeed. Only `quilon test` compiles the blocks. **Run `quilon test` in CI**, or broken
test code passes unnoticed.

## What the run records

A case may ask about the run it is in, through the same functions the harness itself uses —
never a runtime primitive:

| Function | Yields |
|----------|--------|
| `test.casesPassed() -> Num` | Cases that ran with no failing `expect`. |
| `test.casesFailed() -> Num` | Cases that ran with at least one. |
| `test.nestingDepth() -> Num` | How many `describe` groups are open — 0 outside any. |

and the case lifecycle `describe` and `it` drive:

| Function | Effect |
|----------|--------|
| `test.enterSuite() -> Num` | Open a group; yields the depth it sits at. |
| `test.leaveSuite() -> Num` | Close the group just entered; yields the depth that remains. |
| `test.caseFailing() -> Bool` | Whether the running case has already failed an `expect`. Ask **before** closing it — closing clears the mark. |
| `test.finishCase() -> Num` | Close the case, tallying it passed or failed; yields the depth to report it at. |

`test.reportSummary() -> Num` ends the run: the entry point `quilon test` synthesizes calls it
last, and its result is the run's status — 0 passes the suite, anything else fails it.

A suite that imports no harness at all is a compile error at its first `test.describe`, naming the
import that fixes it — never a silent run with no output.
