---
title: "Error messages"
---

# Error messages

Every failure the compiler or a compiled program reports carries a **code**, a message, and
— for a located failure — the source line with the span marked under it. A code is `QN`
and three digits; the first digit names the pipeline family it belongs to — `0` lexer, `1`
parser, `2` module resolution and linking, `3` type checker, `4` codegen and build, `5`
runtime, `6` CLI and usage — and the other two run `x00` upward within it. Each code has a
section below, and `quilon explain QN311` prints that section.

For the program

```quilon ignore
add = (a :: Num) -> Num => < a + true >
```

`quilon check` reports (since `+` is an [overload set](../functions/overloading.md), a
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
[assertion](../corelib/test/README.md) at its own call site, a fail-loud check (a bad
`array[i]`, a computed [range endpoint](../expressions/ranges-and-spread.md#endpoints-are-whole-numbers)
that is a fraction) at the expression that broke the contract. A [failed or unrepresentable
allocation](../memory.md) has no expression to point at and prints its first line alone. A
runtime report carries the source line it names; that line's text is embedded in the
built binary. `core.test`'s `failAt` — which the `Text.replace`/`repeat` contract checks
report through — prints the frame `core.test` composes in Quilon: a `path:line:col:`
position line, the message, and a caret run.

Reports are colored when stderr is a terminal, and plain — the same frame with no escape
sequences — when redirected or under `NO_COLOR` or `TERM=dumb`. A compile error exits 1; a
failing `assert` exits 101.

To stay robust on hostile or machine-generated input, the parser caps how deeply
expressions nest: more than **128 levels** of parentheses, array/record literals, block
statements, `[]T` element types, constructor patterns, or chained prefix operators is
[QN101](#qn101--expression-nesting-too-deep). Ordinary code nests a handful of levels.

## The codes

| Code | Title |
|------|-------|
| QN000 | unreadable source file |
| QN001 | source file with an extension other than `.qn` |
| QN002 | invalid token |
| QN003 | unterminated string literal |
| QN004 | misplaced bidirectional control character |
| QN100 | unexpected token |
| QN101 | expression nesting too deep |
| QN102 | too many parameters |
| QN103 | match with no arms |
| QN104 | interpolation hole with more than one expression |
| QN105 | import path with interpolation |
| QN106 | qualified name through a missing import |
| QN107 | ambiguous `{ }` type declaration |
| QN108 | operator member declared with `:=` |
| QN109 | lowercase sum-type variant |
| QN110 | sum type with fields or a mutating method |
| QN111 | bare expression as a function body |
| QN112 | `>>` where two block closers were meant |
| QN200 | `@` primitive declared outside the corelib |
| QN201 | missing module |
| QN202 | private member reached through its module |
| QN203 | name claimed by an import |
| QN204 | import cycle |
| QN205 | two modules with one name |
| QN206 | module binding used as a value |
| QN207 | ambiguous module prefix |
| QN208 | test blocks without `core.test` |
| QN300 | undefined name |
| QN301 | type mismatch |
| QN302 | call on a data value |
| QN303 | wrong number of arguments |
| QN304 | assignment to an immutable binding |
| QN305 | field write through an immutable binding |
| QN306 | `:=` binding aliasing an immutable value |
| QN307 | `=` binding aliasing a mutable value |
| QN308 | mutating method on an immutable receiver |
| QN309 | mutating method declared with `=` |
| QN310 | duplicate definition |
| QN311 | no matching overload |
| QN312 | ambiguous overload |
| QN313 | overload member with an unannotated parameter |
| QN314 | parameter without a type |
| QN315 | parameter count differs from the function type |
| QN316 | lambda parameter with an open type |
| QN317 | recursive function without a return type |
| QN318 | write to a `Site` field |
| QN319 | misplaced `Site` parameter |
| QN320 | call before the definition |
| QN321 | overload member without a return type |
| QN322 | comparison operator returning a type other than `Bool` |
| QN323 | nested pattern inside a constructor pattern |
| QN324 | non-exhaustive match |
| QN325 | unknown variant |
| QN326 | constructor pattern on a non-sum value |
| QN327 | unsupported `^` signature |
| QN328 | invalid argument to a built-in |
| QN329 | top-level binding that has to be computed |
| QN330 | operator defined at the top level |
| QN331 | operator member with the wrong parameter count |
| QN332 | assertion without a matcher |
| QN333 | `expect` outside a test case |
| QN334 | matcher with the wrong argument count |
| QN335 | matcher on a type outside its reach |
| QN336 | unknown member |
| QN337 | method called as a function |
| QN338 | value with no rendering |
| QN339 | no `^` entry point |
| QN400 | code generation failed |
| QN401 | native build failed |
| QN500 | assertion failed |
| QN501 | index out of bounds |
| QN502 | fractional or unrepresentable range endpoint |
| QN503 | no arm matched |
| QN504 | allocation failed |
| QN505 | reading stdin failed |

## Input

### QN000 — unreadable source file

The file named on the command line is missing or unreadable.

```text
quilon check missing.qn
```

Name an existing `.qn` file, with read permission for the user running the compiler.

### QN001 — source file with an extension other than `.qn`

The file named on the command line, or in a `<< "…"` import, has an extension other than
`.qn`.

```text
quilon run program.ql
```

Rename the file to end in `.qn`; the content stays as it is.

## Lexer

### QN002 — invalid token

A character outside the language's symbol set appears in the source.

```quilon ignore
^ = () -> Num => < # >
```

Remove the character, or write a `~` comment where prose is wanted.

### QN003 — unterminated string literal

A `"` opens a string that reaches the end of the file with no closing `"`.

```quilon ignore
greeting = "hello
```

Close the string on the same line: `greeting = "hello"`.

### QN004 — misplaced bidirectional control character

A Unicode bidirectional control (an embedding, override, isolate, or a scopeless mark)
appears outside a string literal or comment, or opens inside one and reaches the end of that
token unclosed.

```quilon ignore
x = 1 ‮ + 2
```

Keep bidirectional controls inside string literals and comments, and close every opener
before the literal or comment ends.

## Parser

### QN100 — unexpected token

The parser found one token where the grammar requires another — a closing `)`, a name, a
type, a pattern, an expression.

```quilon ignore
^ = () -> Num => < (1 + 2 >
```

The message names both the token found and the one required; supply the required one:
`^ = () -> Num => < (1 + 2) >`.

### QN101 — expression nesting too deep

An expression nests more than 128 levels of parentheses, literals, blocks, element types,
constructor patterns, or prefix operators.

```quilon ignore
^ = () -> Num => < ((((((… 200 levels …)))))) >
```

Split the expression into named bindings.

### QN102 — too many parameters

A function, method, or lambda declares more than 10 parameters.

```quilon ignore
f = (a :: Num, b :: Num, c :: Num, d :: Num, e :: Num, f :: Num, g :: Num, h :: Num, i :: Num, j :: Num, k :: Num) -> Num => < a >
```

Group the parameters into a record type and take that record as one parameter.

### QN103 — match with no arms

A `?` match has no `|` arm — the parser reaches `?` and the arm list that should follow
it turns out empty, a defensive check for a construct the grammar otherwise keeps from
being written. Write at least one arm: `1 ? | 1 => 0 | _ => 1`.

```text
error[QN103]: a match needs at least one `|` arm
```

### QN104 — interpolation hole with more than one expression

A backtick hole inside a string holds more than one expression.

```quilon ignore
^ = () -> Num => < "`1 2`".size >
```

Put one expression in the hole: `` "`1 + 2`" ``.

### QN105 — import path with interpolation

A `<< "…"` import path contains a backtick hole.

```quilon ignore
<< "lib/`name`.qn"
```

Write the path as a plain literal: `<< "lib/util.qn"`.

### QN106 — qualified name through a missing import

A top-level line reaches into a module (`name.member`) with no `<<` for that module above
it. The common case is a test suite missing its harness.

```quilon ignore
test.describe("math", () => < test.it("adds", () => expect(1 + 1, equals(2))) >)
```

Add the import above the line: `<< core.test`.

### QN107 — ambiguous `{ }` type declaration

A `Name = { … }` holds only method-shaped members, so it reads both as a type declaration
and as a record literal.

```quilon ignore
Counter = { bump = => 1 }
```

Add a `::` field to declare a type (`Counter = { n :: Num, bump = => it.n + 1 }`), or give
the members plain values to write a record literal (`counter = { bump = 1 }`).

### QN108 — operator member declared with `:=`

An operator member, or the render member `` ` ``, is declared with `:=`. An operator
yields a value and leaves `it` as it is.

```quilon ignore
Point = { x :: Num, + := (other :: Point) -> Point => < Point { x = it.x + other.x } > }
```

Declare it with `=`: `+ = (other :: Point) -> Point => < … >`.

### QN109 — lowercase sum-type variant

A variant of a sum type starts with a lowercase letter. (The first two variants decide
that a `Name = A / B / …` line is a sum-type declaration — see
[the disambiguation rule](../types/sum-types.md) — so this fires from the third variant on;
a lowercase first or second variant instead reads as dividing undefined names, an
[undefined name](#qn300--undefined-name) error.)

```quilon ignore
Color = Red / Green / blue
```

Capitalize every variant: `Color = Red / Green / Blue`.

### QN110 — sum type with fields or a mutating method

A sum type's trailing `{ }` block declares a `::` field, or a `:=` method. A sum carries
data in its variant payloads and its block holds methods only.

```quilon ignore
Shape = Circle(Num) / Square(Num) { name :: Text }
```

Move the data into a payload (`Circle(Num, Text)`) and keep the block to `=` methods.

### QN111 — bare expression as a function body

A function or method body is a bare expression after `=>`. A lambda's body may be bare;
a declaration's is a block.

```quilon ignore
double = (x :: Num) -> Num => x * 2
```

Write the body as a block: `double = (x :: Num) -> Num => < x * 2 >`.

### QN112 — `>>` where two block closers were meant

Two block closers written together (`>>`) lex as the export marker.

```quilon ignore
^ = () -> Num => < f = () -> Num => < 1 >>
```

Separate them with a space: `< 1 > >`.

## Imports

### QN200 — `@` primitive declared outside the corelib

A user source declares a name starting with `@`. The `@` marks a built-in IO primitive,
which only the corelib defines; user code calls one.

```quilon ignore
@sleep = (seconds :: Num) -> $ => < $ >
```

Declare an ordinary function, and call the corelib's primitive where a primitive is meant.

### QN201 — missing module

A `<<` import names a built-in module the compiler lacks, a file that is missing or
unreadable, or `core.text`, which the compiler merges on its own.

```quilon ignore
<< core.magic
```

Import one of the built-in modules (`core.io`, `core.test`, `core.cli`, `core.time`,
`core.net`, `core.http`, `core.info`) or an existing `.qn` file by path.

### QN202 — private member reached through its module

A qualified reference names a member the module keeps private, or one it lacks.

```quilon ignore
<< core.io
^ = () -> Num => < io.secret() >
```

Reach an exported member, or mark the member `>>` in its module.

### QN203 — name claimed by an import

A binding, parameter, or pattern name is the short name an import binds (`io` for
`<< core.io`).

```quilon ignore
<< core.io
io = 1
```

Rename the binding; the import keeps its name.

### QN204 — import cycle

A module imports itself, directly or through other modules.

```quilon ignore
<< "a.qn"   ~ where a.qn imports this file
```

Move the shared definitions into a third module both import.

### QN205 — two modules with one name

Two imported modules bind the same short name — two files named `util.qn` in different
directories, or a file stem that is unusable as a binding.

```quilon ignore
<< "lib/util.qn"
<< "vendor/util.qn"
```

Rename one of the files, or drop one of the imports.

### QN206 — module binding used as a value

An import's binding name appears where a value is required.

```quilon ignore
<< core.io
^ = () -> Num => < x = io >
```

Reach the module's exports through the binding: `io.print(…)`.

### QN207 — ambiguous module prefix

A short prefix is bound by more than one imported module.

```quilon ignore
<< core.http
<< "vendor/http.qn"
^ = () -> Num => < http.send(request) >
```

Write the full path: `core.http.send(…)`.

### QN208 — test blocks without `core.test`

A file has top-level `test.describe` blocks and `quilon test` finds the harness's summary
function out of scope. Recognizing a call as a test block already requires `<< core.test`
above it — an unimported `test.describe` reads as an ordinary qualified reference and is
[QN106](#qn106--qualified-name-through-a-missing-import) instead — so this is the
backstop for `core.test` itself failing to define its summary function.

```text
error[QN208]: no test harness in scope: `core.test.reportSummary` is undefined
  help: add `<< core.test` above this block
```

## Checker

### QN300 — undefined name

A name is used with no definition above it. Names resolve top to bottom.

```quilon ignore
^ = () -> Num => < total >
```

Define the name before its use: `total = 1` on a line above.

### QN301 — type mismatch

An expression has one type where another is required.

```quilon ignore
x :: Num = "seven"
```

Give the binding a value of the annotated type, or change the annotation to match.

### QN302 — call on a data value

A value of a data type is called with `( )`.

```quilon ignore
^ = () -> Num => < n = 1  n(2) >
```

Call a function, or index a collection with `[ ]`.

### QN303 — wrong number of arguments

A call passes more or fewer arguments than the function declares.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
^ = () -> Num => < double(1, 2) >
```

Pass exactly the declared parameters: `double(1)`.

### QN304 — assignment to an immutable binding

A binding made with `=` is reassigned.

```quilon ignore
^ = () -> Num => < n = 1  n := 2 >
```

Bind it with `:=` to allow writes: `n := 1`.

### QN305 — field write through an immutable binding

A field is written through a binding made with `=`.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p = Point { x = 1 }  p.x := 2 >
```

Bind the record with `:=`: `p := Point { x = 1 }`.

### QN306 — `:=` binding aliasing an immutable value

A `:=` binding takes the value of an `=` binding, a parameter, or the receiver `it` of an
`=` method. A value bound with `=` stays immutable through every alias. Only reference
types are checked — a record, or an array/`Set`/`Map` holding one; an array of `Num`,
`Bool`, or `Text` copies freely and is exempt.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p = Point { x = 1 }  q := p >
```

Bind with `=`, or build a fresh value: `q := Point { x = p.x }`.

### QN307 — `=` binding aliasing a mutable value

An `=` binding takes the value of a `:=` binding; writes through the mutable binding would
change the `=`-bound value. Only reference types are checked, the same as QN306 above.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p := Point { x = 1 }  q = p >
```

Bind with `:=`, or build a fresh value.

### QN308 — mutating method on an immutable receiver

A `:=` method is called on a receiver bound with `=`.

```quilon ignore
Counter = { n :: Num, bump := () -> $ => < it.n := it.n + 1 > }
^ = () -> Num => < c = Counter { n = 0 }  c.bump() >
```

Bind the receiver with `:=`: `c := Counter { n = 0 }`.

### QN309 — mutating method declared with `=`

A method declared with `=` writes to `it`.

```quilon ignore
Counter = { n :: Num, bump = () -> $ => < it.n := it.n + 1 > }
```

Declare it with `:=`: `bump := () -> $ => < … >`. A lambda parameter named `it` inside the
body shadows the receiver; rename it where the write targets the lambda's own value.

### QN310 — duplicate definition

A name is defined twice in one scope, and the two definitions form no overload set.

```quilon ignore
x = 1
x = 2
```

Give each definition its own name.

### QN311 — no matching overload

A call, or an operator, has argument types that match no member of its overload set.
Dispatch is by exact type.

```quilon ignore
^ = () -> Num => < 1 + "x" >
```

Pass the types a member takes; the `help:` line lists the members. To join a number and
text, interpolate: `` "`n`x" ``.

### QN312 — ambiguous overload

More than one member of an overload set matches a call's argument types. Two members with
the very same parameter types are rejected up front, as a
[duplicate definition](#qn310--duplicate-definition) — this is the narrower case a call site
finds ambiguous: a member taking a trailing `site :: Site` the compiler fills in
automatically, alongside one without it, both accept a call that omits it.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (n :: Num, site :: Site) -> Num => < n >
^ = () -> Num => < f(1) >
```

Give the members distinct parameter types, or call with enough arguments to rule one out.

### QN313 — overload member with an unannotated parameter

A member of an overload set leaves a parameter without a type. Exact dispatch reads every
member's full signature.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (t) -> Num => < t.size >
```

Annotate every parameter: `f = (t :: Text) -> Num => < t.size >`.

### QN314 — parameter without a type

A function parameter has no type annotation and no context to take one from.

```quilon ignore
double = (x) => < x * 2 >
```

Annotate the parameter: `double = (x :: Num) => < x * 2 >`.

### QN315 — parameter count differs from the function type

A definition, or a lambda, declares a different number of parameters than the function type
it must match.

```quilon ignore
f :: (Num, Num) -> Num = (a :: Num) => < a >
```

Declare exactly the parameters the type states.

### QN316 — lambda parameter with an open type

A lambda leaves a parameter unannotated in a position that states no function type — an
array element, a plain expression, a sum payload — or an overload set the other arguments
leave open.

```quilon ignore
^ = () -> Num => < fns = [x => x + 1] >
```

Annotate the parameter: `[(x :: Num) => x + 1]`.

### QN317 — recursive function without a return type

A function calls itself and declares no `-> T`. A call to the function needs its return
type before the body is checked.

```quilon ignore
fact = (n :: Num) => < n <= 1 ? 1 : n * fact(n - 1) >
```

Annotate the return type: `fact = (n :: Num) -> Num => < … >`.

### QN318 — write to a `Site` field

A field of a `Site` is assigned. A location is a value.

```quilon ignore
f = (site :: Site) -> $ => < site.line := 1 >
```

Read the fields; build a fresh record where a different location is wanted.

### QN319 — misplaced `Site` parameter

A `Site` parameter sits anywhere but last in a top-level function, or on a lambda or
method. The compiler fills a call site only as the last parameter of a top-level function.

```quilon ignore
check = (site :: Site, n :: Num) -> $ => < $ >
```

Move the `Site` parameter last: `check = (n :: Num, site :: Site) -> $ => < $ >`.

### QN320 — call before the definition

A call reaches an overload set — two or more same-named definitions — every member of
which is defined below the call. A name with only ONE definition below the call is instead
[undefined](#qn300--undefined-name): names resolve top to bottom, and a set forms at its
second member.

```quilon ignore
h = () -> Text => < g(1) >
g = (n :: Num) -> Text => < "a" >
g = (t :: Text) -> Text => < "b" >
```

Move the definitions above the call.

### QN321 — overload member without a return type

A member of an overload set omits its `-> T`. Exact dispatch reads the full signature.

```quilon ignore
f = (n :: Num) => < n >
f = (t :: Text) -> Num => < t.size >
```

Annotate the return type: `f = (n :: Num) -> Num => < n >`.

### QN322 — comparison operator returning a type other than `Bool`

A member for `==`, `!=`, `<`, `<=`, `>`, or `>=` declares a return type other than `Bool`.

```quilon ignore
Point = { x :: Num, == = (other :: Point) -> Num => < 0 > }
```

Return `Bool`: `== = (other :: Point) -> Bool => < it.x == other.x >`.

### QN323 — nested pattern inside a constructor pattern

A constructor pattern's argument is a literal or another constructor. The match dispatches
on the constructor tag alone.

```quilon ignore
^ = () -> Num => < r = Ok(1)  r ? | Ok(1) => 1 | _ => 0 >
```

Bind the payload and compare it in the arm: `| Ok(n) => n == 1 ? 1 : 0`.

### QN324 — non-exhaustive match

A `?` match leaves values uncovered — a sum type with variants no arm lists, or any other
type with no `_` arm.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Red => 1 >
```

Add the missing arms, or a `_` arm.

### QN325 — unknown variant

A constructor pattern names a variant the sum type lacks.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Blue => 1 | _ => 0 >
```

Name one of the type's variants; the message lists them.

### QN326 — constructor pattern on a non-sum value

A constructor pattern matches a value of a type with no variants, or one whose type is
open at the match.

```quilon ignore
^ = () -> Num => < 5 ? | Ok(x) => x | _ => 0 >
```

Match the value itself — a literal, a binding, or `_` — or annotate the value's type.

### QN327 — unsupported `^` signature

The entry point declares parameters other than the supported forms.

```quilon ignore
^ = (n :: Num) -> Num => < n >
```

Declare `^` as `()`, `(args :: []Text)`, or `(args :: []Text, env :: [|Text => Text|])`.

### QN328 — invalid argument to a built-in

A built-in method receives an argument outside its contract that is visible statically —
`Text.repeat` with a negative literal count, an index into a `Map`, an empty `< >` block
where a value is required.

```quilon ignore
^ = () -> Num => < "ab".repeat(-1).size >
```

The message states the contract; pass an argument that meets it.

### QN329 — top-level binding that has to be computed

A top-level `=` binding holds a call, an operator, an array, a record, or `Text`. A
top-level binding becomes a global whose initializer is a constant.

```quilon ignore
total = 1 + 2
^ = () -> Num => < total >
```

Move the computation into `^` or the function that uses it; keep a top-level binding to a
`Num`, `Bool`, or `$` literal, or a function.

### QN330 — operator defined at the top level

An operator symbol names a top-level definition. An operator is a member of the record or
sum type it operates on.

```quilon ignore
+ = (a :: Point, b :: Point) -> Point => < Point { x = a.x + b.x } >
```

Define it inside the type's `{ }`, where `it` is the left operand:
`+ = (other :: Point) -> Point => < … >`.

### QN331 — operator member with the wrong parameter count

An operator member declares more or fewer than one explicit parameter. `it` is the left
operand; the one parameter is the right.

```quilon ignore
Point = { x :: Num, + = (a :: Point, b :: Point) -> Point => < a > }
```

Declare one parameter: `+ = (other :: Point) -> Point => < … >`.

### QN332 — assertion without a matcher

`assert` or `expect` is called with anything but a value and one of the matchers.

```quilon ignore
^ = () -> $ => < assert(1 == 1) >
```

Pass the value and a matcher: `assert(1, equals(1))`. The matchers are `equals`,
`contains`, `not`, `isOk`, `isNotOk`.

### QN333 — `expect` outside a test case

`expect` is called outside an `it` case of a `describe` block.

```quilon ignore
^ = () -> $ => < expect(1, equals(1)) >
```

Use `assert`, which reports and exits, outside a test case.

### QN334 — matcher with the wrong argument count

A matcher is called with more or fewer arguments than it takes.

```quilon ignore
^ = () -> $ => < assert(1, equals(1, 2)) >
```

Pass the matcher's arguments: `equals(1)`.

### QN335 — matcher on a type outside its reach

A matcher meets a type it has no way to inspect: `equals` on a type with no `==`,
`contains` on anything but `Text` or an array, `isOk`/`isNotOk` on anything but a `Result`.

```quilon ignore
^ = () -> $ => < assert(1, contains(1)) >
```

Apply a matcher that reads the type: `assert([1], contains(1))`.

### QN336 — unknown member

`value.name` names a member the value's type lacks. A function of the same name in scope
answers the plain call form only.

```quilon ignore
<< core.io
^ = () -> Num => < n = 1  n.print()  0 >
```

Call the function on the value: `io.print(n)`.

### QN337 — method called as a function

`name(value)` names a member of the value's type, and the top level has no function of that
name.

```quilon ignore
Counter = { n :: Num, bumped = () -> Num => < it.n + 1 > }
^ = () -> Num => < c = Counter { n = 1 }  bumped(c) >
```

Call the member on the value: `c.bumped()`.

### QN338 — value with no rendering

`io.print`, `io.eprint`, or `io.write` receives a value with no `` ` `` render member — a
function.

```quilon ignore
<< core.io
f = () -> Num => < 1 >
^ = () -> Num => < io.print(f)  0 >
```

Print a renderable value, or give the type a `` ` `` member.

### QN339 — no `^` entry point

`run`, `build`, or `compile` is given a file with no `^` function.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
```

Define the entry point: `^ = () -> Num => < 0 >`.

## Code generation and build

### QN400 — code generation failed

The checked program reached a case the code generator lacks. The message names the case.

```text
error[QN400]: unknown array method `frobnicate`
```

File the message and the program as an issue; the checker and the generator agree on every
construct the language reference documents.

### QN401 — native build failed

The linker is missing, or the link or the write of the output failed. The message carries
the linker's own words.

```text
error[QN401]: linker `clang` not found on PATH. Install it, or pass `--linker <name>` (e.g. `--linker gcc`).
```

Install `clang`, or pass `--linker gcc`.

## Runtime

### QN500 — assertion failed

An `assert` or `expect` found its value outside what the matcher accepts. `assert` exits
101; `expect` marks the running case failed and continues.

```quilon ignore
^ = () -> $ => < assert(2 + 2, equals(5)) >
```

The message states what was expected and what was found; fix the program or the
expectation.

### QN501 — index out of bounds

An `array[i]` read has `i` below 0, at or past the array's size, or a fraction.

```quilon ignore
^ = () -> Num => < a = [1, 2]  a[5] >
```

Index within `0` and `a.size - 1`, or check `i < a.size` first.

### QN502 — fractional or unrepresentable range endpoint

A computed `lo <- hi` endpoint is a fraction, NaN, infinite, or beyond the whole numbers a
`Num` holds exactly.

```quilon ignore
^ = () -> Num => <
  half = 0.5
  (1 <- half).size
>
```

Compute whole-number endpoints. (The range expression is written on its own line here — a
`(` that instead followed `half = 0.5` on the SAME line would open a call on `half`, per
[the statement-boundary rule](../expressions/README.md).)

### QN503 — no arm matched

A `?` match reached the end of its arms. The checker proves every match exhaustive; this is
the runtime's backstop.

```text
error[QN503]: no arm of this match matched the value
```

File the program as an issue.

### QN504 — allocation failed

The collector had no memory for the request, or the requested size was outside what a
`Num` represents.

```text
error[QN504]: out of memory allocating 17179869184 bytes
```

Allocate within the machine's memory, and compute sizes that stay whole numbers.

### QN505 — reading stdin failed

`@readStdin` met an IO error on stdin — one other than end of file.

```text
error[QN505]: @readStdin failed: Input/output error (os error 5)
```

Run the program with a readable stdin.
