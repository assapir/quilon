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
| `expect(actual, matcher) -> $` | Report at the call site, mark the running case **failed**, and carry on. Test cases only — see [`expect` is for cases](#expect-is-for-cases). |

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

A **suite** is any `.qn` file with top-level `describe(…)` blocks — a file of nothing but
tests, or the module or program they test ([below](#tests-beside-the-code-costing-a-release-build-nothing)),
with whatever fixtures the cases need. `quilon test` synthesizes the entry point that runs
each block in order; every other command leaves the blocks out of the program. A case checks
itself with `expect`.

```quilon
<< core.test

describe("Text", () => <
  it("trims both ends", () => expect("  padded  ".trim(), equals("padded")))
  it("finds a part", () => expect("haystack", contains("stack")))

  describe("splitting", () => <
    it("splits on a separator", () => expect("a,b,c".split(",").size, equals(3)))
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

### `expect` is for cases

`expect` marks the running **case** failed, and `it` is what closes a case and tallies it — so
an `expect` belongs inside an `it`, inside a `describe`. Anywhere else it is a **compile
error** pointing at `assert`:

- outside a `describe` block there is no reporter at all, the blocks being stripped from
  `run`, `compile`, and `build`;
- inside a `describe` but outside an `it` there is no case to mark, so the failure would be
  printed and never counted.

The rule is lexical, so a top-level helper a case calls uses `assert`, not `expect`.

### Tests beside the code, costing a release build nothing

Tests may sit in the same file as the code they test — beside its `>>` exports, beside its `^`,
or both, as in `examples/tests_alongside_code.qn`. `describe` is the marker; there is no `cfg`
or attribute:

- `check`, `compile`, `build`, `run`: every top-level `describe(…)` is **erased** before the
  checker sees it, so nothing of the harness is type-checked, emitted, or linked. The file's
  own `^` is its entry point and behaves exactly as it would without the blocks. A file whose
  blocks are all it has is no program at all — `compile`, `build`, and `run` pass over it in
  silence rather than reporting a missing entry point.
- `quilon test`: the blocks are **compiled and run**, under the entry point it synthesizes. A
  file's own `^` is not the test run's, so it is ignored rather than called.

Never type-checking them cuts both ways: **a type error inside a `describe` block is invisible
to `check`, `compile`, `build`, and `run`** — they erase the block before the checker sees it
and succeed. Only `quilon test` compiles the blocks. **Run `quilon test` in CI**, or broken
test code passes unnoticed.

### Writing a reporter

What a run looks like is decided in `.qn`, not in the compiler. `describe`, `it` and a failing
`expect` only record what happened; every line of output comes from three functions
`core.test` exports, and a reporter is those three:

| Function | Called |
|----------|--------|
| `reportSuite(name :: Text, depth :: Num) -> $` | On entering a `describe` group, before its body runs. |
| `reportCase(name :: Text, depth :: Num, failed :: Bool) -> $` | Once a case's body has run, `failed` saying which way it went. |
| `reportSummary() -> Num` | Last, from the entry point `quilon test` synthesizes. |

`depth` is **1 for an outermost `describe`**, one more per level of nesting; a case is reported
at the depth of the group holding it. `reportSummary`'s **return value is the process exit
code** — so a run passes only if it returns 0.

The run's state, for the summary and for anything else a reporter wants to say:

| Function | Yields |
|----------|--------|
| `casesPassed() -> Num` | Cases that ran with no failing `expect`. |
| `casesFailed() -> Num` | Cases that ran with at least one. |
| `nestingDepth() -> Num` | How many `describe` groups are open — 0 outside any. |

That is the whole state; the registry primitives behind these three are the harness's own
plumbing, not API. What `core.test` ships is a reporter and nothing more:

```quilon
>> reportSuite = (name :: Text, depth :: Num) -> $ => print("`indent(depth - 1)``name`")

>> reportCase = (name :: Text, depth :: Num, failed :: Bool) -> $ => <
  mark = failed ? red("✗") : green("✓")
  print("`indent(depth)``mark` `name`")
>

>> reportSummary = () -> Num => <
  failed = casesFailed()
  tally = "`casesPassed()` passed, `failed` failed"
  print("")
  print(failed == 0 ? green(tally) : red(tally))
  failed == 0 ? 0 : 1
>
```

**Swapping one in** is not yet possible from a suite: `<< core.test` brings these three names
into scope, and imports are transitive, so a module that defines its own is a duplicate
definition. Selecting a reporter waits on the harness and the default reporter shipping as
separate modules.
