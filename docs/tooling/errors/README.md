---
title: "Error messages"
sidebar:
  order: 0
---

# Error messages

Every failure the compiler or a compiled program reports carries a **code**, a message, and
— for a located failure — the source line with the span marked under it. A code is `QN`
and three digits; the first digit names the pipeline family it belongs to — `0` lexer, `1`
parser, `2` module resolution and linking, `3` type checker, `4` codegen and build, `5`
runtime, `6` CLI and usage — and the other two run `x00` upward within it. Each code has a
section in its family's page, linked from the table below, and `quilon explain QN311`
prints that section.

## Exit codes

A diagnostic's process exit code is its family digit: `quilon run`/`quilon build`/`quilon
check`/`quilon compile` exit with the number below, and a compiled program exits the same
way for a QN5xx it raises at run time (`assert`/`expect` included).

| Family | Codes | Exit code |
|--------|-------|-----------|
| Lexer, parser | QN0xx, QN1xx | 1 |
| Module resolution and linking | QN2xx | 2 |
| Type checker | QN3xx | 3 |
| Codegen and build | QN4xx | 4 |
| Runtime | QN5xx | 5 |

`quilon test`'s own pass/fail status (not a diagnostic) and clap's usage errors are
unrelated to this table and keep their existing exit codes.

For the program

```quilon ignore
add = (a :: Num) -> Num => < a + true >
```

`quilon check` reports (since `+` is an [overload set](../../functions/overloading.md), a
`Num + Bool` matches no member):

```text
error[QN311]: no overload of `+` takes (Num, Bool)
   ╭─[program.qn:1:30]
 1 │ add = (a :: Num) -> Num => < a + true >
   ·                              ┬   ──┬─
   ·                              │     ╰── Bool
   ·                              ╰── Num
   ╰────
  help: the members of `+` are (Num, Num), (Text, Text)
```

The frame is the same everywhere: the code and the message, the file with the 1-based line
and column the report opens at (columns count characters), the source line, a mark under
each span — with a label where one clarifies — and, where a fix is idiomatic, a `help:` line.
A path wider than 60 characters is shown from its end behind a `…`. A failure with no
source location (a missing file, a failed link) prints the first line alone. A multi-line
span marks every line it covers.

Runtime failures use the same frame at the expression responsible: a failing
[assertion](../../corelib/test/README.md) at its own call site, a fail-loud check (a bad
`array[i]`, a computed [range endpoint](../../expressions/ranges-and-spread.md#endpoints-are-whole-numbers)
that is a fraction) at the expression that broke the contract. A [failed or unrepresentable
allocation](../../memory.md) has no expression to point at and prints its first line alone. A
runtime report carries the source line it names; that line's text is embedded in the
built binary. `core.test`'s `failAt` — which the `Text.replace`/`repeat` contract checks
report through — prints the frame `core.test` composes in Quilon: a `path:line:col:`
position line, the message, and a caret run.

Reports are colored when stderr is a terminal, and plain — the same frame with no escape
sequences — when redirected or under `NO_COLOR` or `TERM=dumb`. A compile error and a
failing `assert` each exit with their own code's family digit (see [Exit codes](#exit-codes)
above) — a type error exits 3, a failing `assert` exits 5.

To stay robust on hostile or machine-generated input, the parser caps how deeply
expressions nest: more than **128 levels** of parentheses, array/record literals, block
statements, `[]T` element types, constructor patterns, or chained prefix operators is
[QN101](syntax.md#qn101--expression-nesting-too-deep). Ordinary code nests a handful of levels.

## The codes

An example spanning more than one file adds a fenced block per extra file, titled with
its name relative to the first block (`` ```quilon title="lib/util.qn" ``).

| Code | Title |
|------|-------|
| [QN000](syntax.md#qn000--unreadable-source-file) | unreadable source file |
| [QN001](syntax.md#qn001--source-file-with-an-extension-other-than-qn) | source file with an extension other than `.qn` |
| [QN002](syntax.md#qn002--invalid-token) | invalid token |
| [QN003](syntax.md#qn003--unterminated-string-literal) | unterminated string literal |
| [QN004](syntax.md#qn004--misplaced-bidirectional-control-character) | misplaced bidirectional control character |
| [QN005](syntax.md#qn005--scientific-notation-literal) | scientific notation literal |
| [QN100](syntax.md#qn100--unexpected-token) | unexpected token |
| [QN101](syntax.md#qn101--expression-nesting-too-deep) | expression nesting too deep |
| [QN102](syntax.md#qn102--too-many-parameters) | too many parameters |
| [QN103](syntax.md#qn103--match-with-no-arms) | match with no arms |
| [QN104](syntax.md#qn104--interpolation-hole-with-more-than-one-expression) | interpolation hole with more than one expression |
| [QN105](syntax.md#qn105--import-path-with-interpolation) | import path with interpolation |
| [QN106](syntax.md#qn106--qualified-name-through-a-missing-import) | qualified name through a missing import |
| [QN107](syntax.md#qn107--ambiguous---type-declaration) | ambiguous `{ }` type declaration |
| [QN108](syntax.md#qn108--operator-member-declared-with-) | operator member declared with `:=` |
| [QN109](syntax.md#qn109--lowercase-sum-type-variant) | lowercase sum-type variant |
| [QN110](syntax.md#qn110--sum-type-with-fields-or-a-mutating-method) | sum type with fields or a mutating method |
| [QN111](syntax.md#qn111--bare-expression-as-a-function-body) | bare expression as a function body |
| [QN112](syntax.md#qn112---where-two-block-closers-were-meant) | `>>` where two block closers were meant |
| [QN113](syntax.md#qn113--disallowed-character-glued-to-a-name) | disallowed character glued to a name |
| [QN114](syntax.md#qn114--match-used-as-a-match-arm-body-without-parentheses) | match used as a match-arm body without parentheses |
| [QN115](syntax.md#qn115--a-line-final--closed-a-block-earlier-than-intended) | a line-final `>` closed a block earlier than intended |
| [QN116](syntax.md#qn116--text-pattern-with-interpolation) | text pattern with interpolation |
| [QN200](modules.md#qn200---primitive-declared-outside-the-corelib) | `@` primitive declared outside the corelib |
| [QN201](modules.md#qn201--missing-module) | missing module |
| [QN202](modules.md#qn202--private-member-reached-through-its-module) | private member reached through its module |
| [QN203](modules.md#qn203--name-claimed-by-an-import) | name claimed by an import |
| [QN204](modules.md#qn204--import-cycle) | import cycle |
| [QN205](modules.md#qn205--two-modules-with-one-name) | two modules with one name |
| [QN206](modules.md#qn206--module-binding-used-as-a-value) | module binding used as a value |
| [QN207](modules.md#qn207--ambiguous-module-prefix) | ambiguous module prefix |
| [QN208](modules.md#qn208--test-blocks-without-coretest) | test blocks without `core.test` |
| [QN300](semantics.md#qn300--undefined-name) | undefined name |
| [QN301](semantics.md#qn301--type-mismatch) | type mismatch |
| [QN302](semantics.md#qn302--call-on-a-data-value) | call on a data value |
| [QN303](semantics.md#qn303--wrong-number-of-arguments) | wrong number of arguments |
| [QN304](semantics.md#qn304--assignment-to-an-immutable-binding) | assignment to an immutable binding |
| [QN305](semantics.md#qn305--write-through-an-immutable-binding) | write through an immutable binding |
| [QN306](semantics.md#qn306---binding-aliasing-an-immutable-value) | `:=` binding aliasing an immutable value |
| [QN307](semantics.md#qn307---binding-aliasing-a-mutable-value) | `=` binding aliasing a mutable value |
| [QN308](semantics.md#qn308--mutating-method-on-an-immutable-receiver) | mutating method on an immutable receiver |
| [QN309](semantics.md#qn309--mutating-method-declared-with-) | mutating method declared with `=` |
| [QN310](semantics.md#qn310--duplicate-definition) | duplicate definition |
| [QN311](semantics.md#qn311--no-matching-overload) | no matching overload |
| [QN312](semantics.md#qn312--ambiguous-overload) | ambiguous overload |
| [QN313](semantics.md#qn313--overload-member-with-an-unannotated-parameter) | overload member with an unannotated parameter |
| [QN314](semantics.md#qn314--parameter-without-a-type) | parameter without a type |
| [QN315](semantics.md#qn315--parameter-count-differs-from-the-function-type) | parameter count differs from the function type |
| [QN316](semantics.md#qn316--lambda-parameter-with-an-open-type) | lambda parameter with an open type |
| [QN317](semantics.md#qn317--recursive-function-without-a-return-type) | recursive function without a return type |
| [QN318](semantics.md#qn318--write-to-a-site-field) | write to a `Site` field |
| [QN319](semantics.md#qn319--misplaced-site-parameter) | misplaced `Site` parameter |
| [QN320](semantics.md#qn320--call-before-the-definition) | call before the definition |
| [QN321](semantics.md#qn321--overload-member-without-a-return-type) | overload member without a return type |
| [QN322](semantics.md#qn322--comparison-operator-returning-a-type-other-than-bool) | comparison operator returning a type other than `Bool` |
| [QN323](semantics.md#qn323--nested-pattern-inside-a-constructor-pattern) | nested pattern inside a constructor pattern |
| [QN324](semantics.md#qn324--non-exhaustive-match) | non-exhaustive match |
| [QN325](semantics.md#qn325--unknown-variant) | unknown variant |
| [QN326](semantics.md#qn326--constructor-pattern-on-a-non-sum-value) | constructor pattern on a non-sum value |
| [QN327](semantics.md#qn327--unsupported--signature) | unsupported `^` signature |
| [QN328](semantics.md#qn328--invalid-argument-to-a-built-in) | invalid argument to a built-in |
| [QN329](semantics.md#qn329--mutable-global-exported) | mutable global exported |
| [QN330](semantics.md#qn330--operator-defined-at-the-top-level) | operator defined at the top level |
| [QN331](semantics.md#qn331--operator-member-with-the-wrong-parameter-count) | operator member with the wrong parameter count |
| [QN332](semantics.md#qn332--assertion-without-a-matcher) | assertion without a matcher |
| [QN333](semantics.md#qn333--expect-outside-a-test-case) | `expect` outside a test case |
| [QN334](semantics.md#qn334--matcher-with-the-wrong-argument-count) | matcher with the wrong argument count |
| [QN335](semantics.md#qn335--matcher-on-a-type-outside-its-reach) | matcher on a type outside its reach |
| [QN336](semantics.md#qn336--unknown-member) | unknown member |
| [QN337](semantics.md#qn337--method-called-as-a-function) | method called as a function |
| [QN338](semantics.md#qn338--value-with-no-rendering) | value with no rendering |
| [QN339](semantics.md#qn339--no--entry-point) | no `^` entry point |
| [QN340](semantics.md#qn340--static-call-on-a-method-that-reads-its-receiver) | static call on a method that reads its receiver |
| [QN341](semantics.md#qn341--store-into-a-mutable-container-aliasing-an-immutable-value) | store into a mutable container aliasing an immutable value |
| [QN342](semantics.md#qn342--missing-constructor-field) | missing constructor field |
| [QN343](semantics.md#qn343--unknown-constructor-field) | unknown constructor field |
| [QN344](semantics.md#qn344--reserved-name) | reserved name |
| [QN345](semantics.md#qn345--record-field-with-a-function-type) | record field with a function type |
| [QN346](semantics.md#qn346--unsupported-sum-type-payload) | unsupported sum-type payload |
| [QN347](semantics.md#qn347--atomic-binding-declared-without-) | atomic binding declared without `:=` |
| [QN348](semantics.md#qn348--atomic-binding-used-with--after-its-declaration) | atomic binding used with `@` after its declaration |
| [QN349](semantics.md#qn349--atomic-binding-reassignment-waits-on-a-deferred-value) | atomic binding reassignment waits on a deferred value |
| [QN350](semantics.md#qn350---value-shared-across-fibers) | `:=` value shared across fibers |
| [QN351](semantics.md#qn351--unresolved-result-payload) | unresolved `Result` payload |
| [QN352](semantics.md#qn352--top-level-function-never-called) | top-level function never called |
| [QN353](semantics.md#qn353--top-level-function-reachable-only-from-test-blocks) | top-level function reachable only from test blocks |
| [QN400](build.md#qn400--code-generation-failed) | code generation failed |
| [QN401](build.md#qn401--native-build-failed) | native build failed |
| [QN500](runtime.md#qn500--assertion-failed) | assertion failed |
| [QN501](runtime.md#qn501--index-out-of-bounds) | index out of bounds |
| [QN502](runtime.md#qn502--fractional-or-unrepresentable-range-endpoint) | fractional or unrepresentable range endpoint |
| [QN503](runtime.md#qn503--no-arm-matched) | no arm matched |
| [QN504](runtime.md#qn504--allocation-failed) | allocation failed |
| [QN505](runtime.md#qn505--reading-stdin-failed) | reading stdin failed |
| [QN506](runtime.md#qn506--empty-from-in-replacereplaceall) | empty `from` in `replace`/`replaceAll` |
| [QN507](runtime.md#qn507--stack-overflow) | stack overflow |
| [QN508](runtime.md#qn508--invalid-write-file-descriptor) | invalid write file descriptor |
| [QN509](runtime.md#qn509--write-failed) | write failed |
| [QN510](runtime.md#qn510--invalid-replace-count) | invalid `replace` count |
| [QN511](runtime.md#qn511--bind-failed) | bind failed |
