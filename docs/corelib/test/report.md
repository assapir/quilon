# `core.test.report` — the test harness

The **harness** (`describe`, `it`) groups checks into named cases that `quilon test` runs
and reports. `<< core.test.report` pulls in [`core.test`](README.md), so a suite needs
nothing else. Every name it defines is ordinary `.qn`, which is what makes the output
[replaceable](#writing-a-reporter).

See the [corelib index](../README.md), `examples/test_suite.qn`, and
`examples/custom_test_reporter.qn`.

A **suite** is any `.qn` file with top-level `describe(…)` blocks — a file of nothing but
tests, or the module or program they test ([below](#tests-beside-the-code-costing-a-release-build-nothing)),
with whatever fixtures the cases need. `quilon test` synthesizes the entry point that runs
each block in order; every other command leaves the blocks out of the program. A case checks
itself with `expect`.

```quilon
<< core.test.report

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

Both come from `core.test.report`, and neither is privileged: the compiler recognizes a
top-level `describe(…)` call **by name**, so a `describe`/`it` you define yourself drives the
same machinery — see [Writing a reporter](#writing-a-reporter).

The **exit code** is 0 only when every case in every suite passed, so `quilon test` drops
straight into CI. A suite that fails to compile — or to parse — counts as a failed suite.

The case tree and the summary go to **stdout**; a failing assertion's
[error frame](../../tooling/errors.md) goes to **stderr**, like every other compiler
diagnostic, so each stream reads on its own when they are captured separately.

Suites run one process each, so a failure in one does not stop the others.

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

`expect` marks the running **case** failed, and `it` is what closes a case and tallies it — so
an `expect` belongs inside an `it`, inside a `describe`. Anywhere else it is a **compile
error** pointing at `assert`:

- outside a `describe` block there is no reporter at all, the blocks being stripped from
  `run`, `compile`, and `build`;
- inside a `describe` but outside an `it` there is no case to mark, so the failure would be
  printed and never counted.

The rule is lexical, so a top-level helper a case calls uses `assert`, not `expect`.

## Tests beside the code, costing a release build nothing

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

## Test-only imports

The blocks need the harness; the code beside them does not. Import it with **`<<?`** and it
follows the blocks — `quilon test` resolves it, every other command erases it:

```quilon ignore
<< core.io             ~ the program uses this
<<? core.test.report   ~ only the blocks use this

>> slugify = (title :: Text) -> Text => title.trim().toLower().replaceAll(" ", "-")

describe("slugify", () => <
  it("hyphenates", () => expect(slugify("Hello World"), equals("hello-world")))
>)
```

A `<<?` module reaches **no build**: nothing of it is checked, emitted, or linked. And it is
**never merged into an importer's scope** — `<< core.http` brings that module's own code, not
the reporter its own suite runs under, so a program is free to define a `green`, an `it` or a
`describe` of its own.

The names are the blocks' to use. Reading one from ordinary code — a fixture included — is a
compile error under `check`, `compile`, `build`, and `run`, naming the import:

```
error: `green` comes in through `<<? core.test.report`, a test-only import: …
```

Nothing else resolves the import, so the **path** is checked by `quilon test` alone — one more
reason to run it in CI.

`examples/tests_alongside_code.qn` is the shipped example.

## Writing a reporter

What a run looks like is decided in `.qn`, not in the compiler. `describe`, `it` and a failing
`expect` only record what happened; every line of output comes from three functions, and a
reporter is those three:

| Function | Called |
|----------|--------|
| `reportSuite(name :: Text, depth :: Num) -> $` | On entering a `describe` group, before its body runs. |
| `reportCase(name :: Text, depth :: Num, failed :: Bool) -> $` | Once a case's body has run, `failed` saying which way it went. |
| `reportSummary() -> Num` | Last, from the entry point `quilon test` synthesizes. |

`depth` is **1 for an outermost `describe`** and one more per level of nesting; a case is
reported at the depth of the group holding it. **`reportSummary`'s result is the run's status:**
0 passes the suite, anything else fails it — which is what `quilon test` exits non-zero on.

To swap in your own, import **`core.test`** instead of `core.test.report` and define all five
names — the two harness functions as well, since `describe` and `it` are what call
`reportSuite` and `reportCase`. `quilon test` binds `reportSummary` **by name** in the linked
program, so yours is the one that ends the run; and a top-level `describe(…)` is recognized by
name too, so your `describe` marks test blocks exactly as the shipped one does.

`core.test` gives you everything the run records and the case lifecycle to drive — see
[what the run records](README.md#what-the-run-records); a reporter never names a runtime
primitive.

A complete replacement — one line per case in TAP order, no indentation, no color. This is
`examples/custom_test_reporter.qn` cut down to two cases; under `quilon test` it reports
exactly the lines below the snippet (after the suite's path, which the runner prints):

```quilon
<< core.io
<< core.test

reportSuite = (name :: Text, depth :: Num) -> $ => $

reportCase = (name :: Text, depth :: Num, failed :: Bool) -> $ =>
  print("`failed ? "not ok" : "ok"` `casesPassed() + casesFailed()` - `name`")

reportSummary = () -> Num => <
  print("1..`casesPassed() + casesFailed()`")
  casesFailed() == 0 ? 0 : 1
>

describe = (name :: Text, body :: () -> $) -> $ => <
  reportSuite(name, enterSuite())
  body()
  leaveSuite()
  $
>

it = (name :: Text, body :: () -> $) -> $ => <
  body()
  failed = caseFailing()
  reportCase(name, finishCase(), failed)
>

describe("Text", () => <
  it("trims both ends", () => expect("  padded  ".trim(), equals("padded")))
  it("finds a part", () => expect("haystack", contains("stack")))
>)
```

```
ok 1 - trims both ends
ok 2 - finds a part
1..2
```

Nothing is re-exported here, so the five names may stay module-private (no `>>`) when the
reporter lives in the suite itself; put them in their own module with `>>` to share one
reporter across suites, and import that module instead of `core.test.report`.

A suite that imports neither `core.test.report` nor a reporter of its own is a compile error at
its first `describe`, naming the import that fixes it — never a silent run with no output.
