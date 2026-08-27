# `core.test` — assertions and checks

**Assertions** (`assert` / `expect`) make a program verify itself as it runs — what every
example in `examples/` does. They are **compiler-provided**, like `print`: no import,
`core.test` included. The module itself holds what checks and reporters are built from:
`failAt`, [the run's recorded state, and the case lifecycle](#what-the-run-records). The
**harness** that groups checks into named cases (`describe` / `it`) lives in
[`core.test.report`](report.md).

See the [corelib index](../README.md) and `examples/assert_demo.qn`.

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
| `expect(actual, matcher) -> $` | Report at the call site, mark the running case **failed**, and carry on. Test cases only — see [`expect` is for cases](report.md#expect-is-for-cases). |

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
| `failAt(message :: Text) -> $` | Report `message` at the caller's location and exit `101` — the same frame `assert` uses. Take a trailing [`site :: Site`](../../functions/site.md) and forward it, and the report blames ITS caller. From `core.test`. |

```quilon
<< core.test

assertEven = (n :: Num, site :: Site) -> $ =>
  n % 2 == 0 ? $ : failAt("`n` is odd", site)
```

## What the run records

The rest of `core.test` is what [a reporter of your own](report.md#writing-a-reporter) is
built from — a reporter never names a runtime primitive:

| Function | Yields |
|----------|--------|
| `casesPassed() -> Num` | Cases that ran with no failing `expect`. |
| `casesFailed() -> Num` | Cases that ran with at least one. |
| `nestingDepth() -> Num` | How many `describe` groups are open — 0 outside any. |

and the case lifecycle a `describe`/`it` of your own drives:

| Function | Effect |
|----------|--------|
| `enterSuite() -> Num` | Open a group; yields the depth it sits at. |
| `leaveSuite() -> Num` | Close the group just entered; yields the depth that remains. |
| `caseFailing() -> Bool` | Whether the running case has already failed an `expect`. Ask **before** closing it — closing clears the mark. |
| `finishCase() -> Num` | Close the case, tallying it passed or failed; yields the depth to report it at. |

`core.test` defines no `describe` / `it` / `report*`, which is what leaves those names free
for [a reporter of your own](report.md#writing-a-reporter).
