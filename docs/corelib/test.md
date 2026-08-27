# Assertions and the test harness

**Assertions** (`assert` / `expect`) make a program verify itself as it runs — what every
example in `examples/` does. They are **compiler-provided**, like `print`: no import. The
**harness** (`describe`, `it`) groups those checks into named cases that `quilon test` runs
and reports, and comes from `core.test` (`<< core.test`).

See the [corelib index](../LANGUAGE.md#corelib), `examples/assert_demo.qn`, and
`examples/test_suite.qn`.

## Assertions

An assertion takes the **value under test first** and a **matcher second**:

```quilon
assert(2 + 2, equals(4))
expect(response, isOk())
```

Two entry points, one vocabulary. They differ only in what a FAILURE does:

| Function | On failure |
|----------|-----------|
| `assert(actual, matcher) -> $` | Report at the call site and **exit 101** (the Rust-panic convention). For examples and ordinary code. |
| `expect(actual, matcher) -> $` | Report at the call site, mark the running case **failed**, and carry on. Test code only — see [`expect` is for tests](#expect-is-for-tests). |

A holding assertion does nothing. A failure reports in the standard
[error frame](../LANGUAGE.md#error-messages) at **your** call site — the line the assertion
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
| `equals(expected)` | `actual == expected`, through the [`==` member](../LANGUAGE.md#overloading) — so `Num`/`Text`/`Bool` and any user record or sum that declares one. |
| `contains(part)` | A `Text` has `part` as a substring, or an array has an element equal to it (again through the element type's `==`). |
| `not(matcher)` | The matcher it wraps does not hold. Composes around any of them. |
| `isOk()` / `isNotOk()` | A [`Result`](../LANGUAGE.md#result) is `Ok` / `NotOk`. |

```quilon
assert(6 * 7, equals(42))
assert("assertions and matchers", contains("matcher"))
assert([2, 4, 6], not(contains(5)))
assert([10, 20].at(0), isOk())       ~ Ok in bounds
assert([10, 20].at(9), isNotOk())    ~ NotOk out of bounds
```

Both values in a report are
[rendered](../LANGUAGE.md#string-interpolation-and-the-render-operator-) — `Num`/`Text`/`Bool`
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
| `failAt(message :: Text) -> $` | Report `message` at the caller's location and exit `101` — the same frame `assert` uses. Take a trailing [`site :: Site`](../LANGUAGE.md#call-site-locations--site) and forward it, and the report blames ITS caller. From `core.test`. |

```quilon
<< core.test

assertEven = (n :: Num, site :: Site) -> $ =>
  n % 2 == 0 ? $ : failAt("`n` is odd", site)
```

## The test harness

A **suite** is a `.qn` file with top-level `describe(…)` blocks and no `^` — it may declare
whatever fixtures its cases need. `quilon test` synthesizes the entry point that runs each
block in order; every other command leaves the blocks out of the program. A case checks
itself with `expect`.

```quilon
<< core.test

describe("Text", () => <
  it("trims both ends", () => expect("  padded  ".trim(), equals("padded")))
  it("finds a part", () => expect("haystack", contains("stack")))

  describe("splitting", () => <
    it("splits on a separator", () => expect("a,b,c".split(",").size, equals(3)))
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

3 passed, 0 failed
```

| Function | Effect |
|----------|--------|
| `describe(name :: Text, body :: () -> $) -> $` | A group of cases. Nestable — the report indents by depth. `body` runs immediately. |
| `it(name :: Text, body :: () -> $) -> $` | One case, reported once `body` has run, `✓` or `✗`. |

The **exit code** is 0 only when every case in every suite passed, so `quilon test` drops
straight into CI. A suite that fails to compile — or to parse — counts as a failed suite.

The case tree and the summary go to **stdout**; a failing assertion's
[error frame](../LANGUAGE.md#error-messages) goes to **stderr**, like every other compiler
diagnostic, so each stream reads on its own when they are captured separately.

Suites run one process each, so a failure in one does not stop the others.

### A failing case does not stop the run

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

### `expect` is for tests

`expect` records its failure with the reporter, and only a `describe` block has one — the
blocks are stripped from `run`, `compile`, and `build`, so an `expect` in ordinary code would
have nothing to record into. Writing one outside a `describe` block is a **compile error**
pointing at `assert`, rather than a program that silently drops its failures. (The rule is
lexical: an `expect` belongs inside a `describe`, not in a top-level helper a case calls.)

### Blocks as arguments

A `< >` block closes on a [line-final `>`](../LANGUAGE.md#expressions), so a lambda
with a block body puts the call's closing `)` on the next line. Writing each `it` as a single
expression keeps that to the `describe` alone.

### Suites cost a release build nothing

`describe` is the marker — there is no `cfg` or attribute. A top-level `describe(…)` call is
test code, so `run`, `compile`, and `build` never type-check or emit it — nothing of the
harness reaches the binary. And a file with test blocks but no `^` is not a compilation unit
at all: those three pass over it in silence rather than reporting a missing entry point.
Tests can therefore sit in the file they test.

### Reporters

What a run looks like is decided in `.qn`, not in the compiler. `describe`, `it`, and a
failing `expect` record what happened through a reporter-agnostic registry of `__test_*`
primitives — nesting depth, a per-case failed mark, and two counts, no rendering — and all
rendering lives in three functions `core.test` exports:

| Function | Called when |
|----------|-------------|
| `reportSuite(name :: Text, depth :: Num) -> $` | A `describe` group is entered. |
| `reportCase(name :: Text, depth :: Num, failed :: Bool) -> $` | A case has run, `failed` saying which way. |
| `reportSummary() -> Num` | Last, from the synthesized entry point. Prints the tally and returns the exit code. |

A reporter of its own defines the same three; selecting it is a matter of pointing the
synthesized entry at another module's `reportSummary`.
