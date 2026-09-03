---
title: "core.test — the test harness, assertions and checks"
sidebar:
  label: "core.test"
  order: 2
---

# `core.test` — the test harness, assertions and checks

**Assertions** (`assert` / `expect`) make a program verify itself as it runs — what every
example in `examples/` does. They are **compiler-provided** and available in every program
without an import. The **harness** that groups checks into named cases
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

Two entry points, one vocabulary. They differ in what a FAILURE does:

| Function | On failure |
|----------|-----------|
| `assert(actual, matcher) -> $` | Report at the call site and **exit 101**. For examples and ordinary code. |
| `expect(actual, matcher) -> $` | Report at the call site, mark the running case **failed**, and carry on. Test cases only — see [`expect` is for cases](#expect-is-for-cases). |

A holding assertion has no effect. A failure reports in the standard
[error frame](../../tooling/errors.md) at the assertion's own call site — the line the
assertion is written on, inside a helper as much as inside `^`:

```text
error[QN500]: assertion failed: expected 41, got 42
   ╭─[demo.qn:4:3]
 4 │   assert(6 * 7, equals(41))
   ·   ─────────────────────────
   ╰────
```

### The matchers

| Matcher | Holds when |
|---------|-----------|
| `equals(expected)` | `actual == expected`, through the [`==` member](../../functions/overloading.md): `Num`/`Text`/`Bool`, and any record or sum that declares one. |
| `contains(part)` | A `Text` has `part` as a substring, or an array has an element equal to it (through the element type's `==`). |
| `not(matcher)` | The wrapped matcher fails to hold. Composes around any matcher. |
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
directly, records, sum types and arrays through their `` ` `` operator — and a `Text` is
quoted, so a trailing space or an empty string is visible. Applying a matcher to a type it
reads nothing from — `equals` on a type without a `==` member, `contains` on a `Num`, `isOk`
on a sum without that variant — is a compile error naming the missing member.

The matchers are compiler-provided. They compose with one another, and
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
tests, or the module or program they test ([below](#tests-beside-the-code)),
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

The compiler recognizes a top-level `test.describe(…)` call **by name**. The report takes one
of two forms, chosen with [`--reporter`](#selecting-cases-and-choosing-the-reporter).

The **exit code** is 0 when every case in every suite passed. A suite that fails to compile
— or to parse — counts as a failed suite.

The case tree and the summary go to **stdout**; a failing assertion's
[error frame](../../tooling/errors.md) goes to **stderr**, like every other compiler
diagnostic.

Suites run one process each; a failing suite leaves the others running. A suite without
`<< core.test` is a compile error at its first `test.describe`, naming the import.

## Selecting cases and choosing the reporter

`quilon test` takes these options:

| Option | Effect |
|--------|--------|
| `--reporter human` | The report above: the case tree on stdout, then the summary line. The default. |
| `--reporter json` | One JSON object per event on stdout, one object per line — [the events below](#the-json-events). |
| `--only <path>` | Run the suite or case at `path` and pass over the rest. Repeatable; each occurrence adds a path. Given with one suite file. |
| `--binary <out>` | Build the suite into a native, debuggable executable at `out` instead of running it. See [Building a debuggable test binary](#building-a-debuggable-test-binary). |

### Paths

A path is the names from the outermost `describe` down to a suite or a case, joined by `/`.
In the suite above, `Text/splitting` names the nested group and
`Text/splitting/splits on a separator` its first case. A name is a text literal; `/` is the
separator.

A case path selects that case. A suite path selects every case under it. A `describe` or
`it` with a selected case under it runs; every other one is passed over — body and all —
with no event and no output. The summary counts the cases that ran.

```bash
quilon test tests/text.qn --only "Text/splitting"                   # every case in the group
quilon test tests/text.qn --only "Text/trims both ends" --only "Text/finds a part"
```

`--only` is checked against the file's paths before the run. A path the file lacks is an
error on stderr that lists the file's paths, and the run exits 1.

### Building a debuggable test binary

`--binary <out>` builds the suite into a native executable at `out` instead of running it,
always with DWARF debug info, so a debugger (`gdb`/`lldb`) can step through a case — see
[Compiling & running](../../tooling/compiling.md) for the full command and its output.
Combined with `--only`, the excluded `describe`/`it` blocks are dropped before code
generation, so `out` alone reproduces the filtered run — the shape a debugger's launch
configuration wants.

```bash
quilon test suite.qn --only "Suite/one case" --binary suite_debug
gdb ./suite_debug
```

### The JSON events

Under `--reporter json`, stdout carries the run's events and nothing else, one JSON object
per line, in the order they happen. A failing `expect`'s
[error frame](../../tooling/errors.md) goes to stderr under both reporters. The schema is
stable.

| Event | Fields | Written when |
|-------|--------|--------------|
| `suite` | `path` — the suite's path; `depth` — the count of enclosing suites, `0` for an outermost one. | A `describe` opens. |
| `case` | `path` — the case's path; `status` — `"pass"` or `"fail"`. With `"fail"`: `message` — the first failing `expect`'s message; `file` — its source file, as the compiler resolved it; `line` — its line number. | A case closes. |
| `summary` | `passed`, `failed` — the run's totals. | The run ends. |

```json
{"event":"suite","path":"Text","depth":0}
{"event":"case","path":"Text/trims both ends","status":"pass"}
{"event":"suite","path":"Text/splitting","depth":1}
{"event":"case","path":"Text/splitting/splits on a separator","status":"fail","message":"assertion failed: expected 4, got 3","file":"tests/text.qn","line":9}
{"event":"summary","passed":1,"failed":1}
```

## A failing case and the run

The first failing `expect` in a case **ends that case**: the assertions after it, and their
subjects, are left unevaluated, and the suite continues with the next case. Every case is
reported the way it went, and the summary counts every case:

```
arithmetic
  ✓ holds
  ✗ does not hold
  ✓ runs after the failure

2 passed, 1 failed
```

`assert` inside a case is fatal and ends the run where it failed; it suits a precondition
the rest of the case depends on.

## `expect` is for cases

`expect` marks the running **case** failed, and `test.it` is what closes a case and tallies
it. An `expect` belongs inside a `test.it`, inside a `test.describe`; anywhere else it is a
**compile error** pointing at `assert`. The rule is lexical: a top-level helper a case calls
uses `assert`.

## Tests beside the code

Tests may sit in the same file as the code they test — beside its `>>` exports, beside its `^`,
or both, as in `examples/tests_alongside_code.qn`. `describe` is the marker:

- `check`, `compile`, `build`, `run`: every top-level `test.describe(…)` is **erased** before the
  checker sees it. A file whose blocks are all it has is passed over by `compile`, `build`, and
  `run` in silence.
- `quilon test`: the blocks are **compiled and run**, under the entry point it synthesizes. The
  file's own `^` is ignored.

The `<< core.test` the blocks need takes no marker of its own. A function nothing reaches is
left out of the build, so the harness is emitted with the blocks it serves and with nothing
else. The shaking is over EMISSION: an imported module is resolved and type-checked, and the
names it exports occupy the importer's scope, so a `describe` of the program's own beside
`<< core.test` is a duplicate definition.

`check`, `compile`, `build`, and `run` erase a `describe` block before the checker sees it, so
a type error inside one is reported by `quilon test` alone.

## What the run records

A case may ask about the run it is in, through the same functions the harness itself uses:

| Function | Yields |
|----------|--------|
| `test.casesPassed() -> Num` | Cases that ran with no failing `expect`. |
| `test.casesFailed() -> Num` | Cases that ran with at least one. |
| `test.nestingDepth() -> Num` | How many `describe` groups are open — 0 outside any. |

and the case lifecycle `describe` and `it` drive:

| Function | Effect |
|----------|--------|
| `test.enterSuite(name :: Text) -> Num` | Open the group `name` and report it; yields the depth it sits at. |
| `test.leaveSuite() -> Num` | Close the innermost open group; yields the depth that remains. |
| `test.caseFailing() -> Bool` | Whether the running case has already failed an `expect`. Closing the case clears the mark. |
| `test.finishCase(name :: Text) -> Num` | Close the case `name`, tallying it passed or failed and reporting it; yields the depth it sits at. |

`test.reportSummary() -> Num` ends the run: the entry point `quilon test` synthesizes calls it
last, and its result is the run's status — 0 passes the suite, anything else fails it.

Each of these reports its event through the compiler, which renders it per the chosen
[reporter](#selecting-cases-and-choosing-the-reporter).
