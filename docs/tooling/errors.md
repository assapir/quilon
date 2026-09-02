---
title: "Error messages"
---

# Error messages

Every failure the compiler or a compiled program reports carries a **code**, a message, and
— for a located failure — the source line with the span marked under it. The codes run
`Q000` upward in pipeline order (input, lexer, parser, imports, checker, code generation,
runtime); each has a section below, and `quilon explain Q038` prints that section.

For the program

```quilon ignore
add = (a :: Num) -> Num => < a + true >
```

`quilon check` reports (since `+` is an [overload set](../functions/overloading.md), a
`Num + Bool` matches no member):

```text
error[Q038]: no overload of `+` takes (Num, Bool)
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
`array[i]`, a computed [range endpoint](../expressions/ranges-and-spread.md#endpoints-must-be-whole-numbers)
that is a fraction) at the expression that broke the contract. A [failed or unrepresentable
allocation](../memory.md) has no expression to point at and prints its first line alone. A
runtime report carries the source line it names, so that line's text is embedded in the
built binary. `core.test`'s `failAt` — which the `Text.replace`/`repeat` contract checks
report through — prints the frame `core.test` composes in Quilon: a `path:line:col:`
position line, the message, and a caret run.

Reports are colored when stderr is a terminal, and plain — the same frame with no escape
sequences — when redirected or under `NO_COLOR` or `TERM=dumb`. A compile error exits 1; a
failing `assert` exits 101.

To stay robust on hostile or machine-generated input, the parser caps how deeply
expressions nest: more than **128 levels** of parentheses, array/record literals, block
statements, `[]T` element types, constructor patterns, or chained prefix operators is
[Q006](#q006--expression-nesting-too-deep). Ordinary code nests a handful of levels.

## The codes

| Code | Title |
|------|-------|
| Q000 | unreadable source file |
| Q001 | source file with an extension other than `.qn` |
| Q002 | invalid token |
| Q003 | unterminated string literal |
| Q004 | misplaced bidirectional control character |
| Q005 | unexpected token |
| Q006 | expression nesting too deep |
| Q007 | too many parameters |
| Q008 | match with no arms |
| Q009 | interpolation hole with more than one expression |
| Q010 | import path with interpolation |
| Q011 | qualified name through a missing import |
| Q012 | ambiguous `{ }` type declaration |
| Q013 | operator member declared with `:=` |
| Q014 | lowercase sum-type variant |
| Q015 | sum type with fields or a mutating method |
| Q016 | bare expression as a function body |
| Q017 | `>>` where two block closers were meant |
| Q018 | `@` primitive declared outside the corelib |
| Q019 | missing module |
| Q020 | private member reached through its module |
| Q021 | name claimed by an import |
| Q022 | import cycle |
| Q023 | two modules with one name |
| Q024 | module binding used as a value |
| Q025 | ambiguous module prefix |
| Q026 | test blocks without `core.test` |
| Q027 | undefined name |
| Q028 | type mismatch |
| Q029 | call on a data value |
| Q030 | wrong number of arguments |
| Q031 | assignment to an immutable binding |
| Q032 | field write through an immutable binding |
| Q033 | `:=` binding aliasing an immutable value |
| Q034 | `=` binding aliasing a mutable value |
| Q035 | mutating method on an immutable receiver |
| Q036 | mutating method declared with `=` |
| Q037 | duplicate definition |
| Q038 | no matching overload |
| Q039 | ambiguous overload |
| Q040 | overload member with an unannotated parameter |
| Q041 | parameter without a type |
| Q042 | parameter count differs from the function type |
| Q043 | lambda parameter with an open type |
| Q044 | recursive function without a return type |
| Q045 | write to a `Site` field |
| Q046 | misplaced `Site` parameter |
| Q047 | call before the definition |
| Q048 | overload member without a return type |
| Q049 | comparison operator returning a type other than `Bool` |
| Q050 | nested pattern inside a constructor pattern |
| Q051 | non-exhaustive match |
| Q052 | unknown variant |
| Q053 | constructor pattern on a non-sum value |
| Q054 | unsupported `^` signature |
| Q055 | invalid argument to a built-in |
| Q056 | top-level binding that has to be computed |
| Q057 | operator defined at the top level |
| Q058 | operator member with the wrong parameter count |
| Q059 | assertion without a matcher |
| Q060 | `expect` outside a test case |
| Q061 | matcher with the wrong argument count |
| Q062 | matcher on a type outside its reach |
| Q063 | unknown member |
| Q064 | method called as a function |
| Q065 | value with no rendering |
| Q066 | no `^` entry point |
| Q067 | code generation failed |
| Q068 | native build failed |
| Q069 | assertion failed |
| Q070 | index out of bounds |
| Q071 | fractional or unrepresentable range endpoint |
| Q072 | no arm matched |
| Q073 | allocation failed |
| Q074 | reading stdin failed |

## Input

### Q000 — unreadable source file

The file named on the command line is missing or unreadable.

```text
quilon check missing.qn
```

Name an existing `.qn` file, with read permission for the user running the compiler.

### Q001 — source file with an extension other than `.qn`

The file named on the command line, or in a `<< "…"` import, has an extension other than
`.qn`.

```text
quilon run program.ql
```

Rename the file to end in `.qn`; the content stays as it is.

## Lexer

### Q002 — invalid token

A character outside the language's symbol set appears in the source.

```quilon ignore
^ = () -> Num => < # >
```

Remove the character, or write a `~` comment where prose is wanted.

### Q003 — unterminated string literal

A `"` opens a string that reaches the end of the file with no closing `"`.

```quilon ignore
greeting = "hello
```

Close the string on the same line: `greeting = "hello"`.

### Q004 — misplaced bidirectional control character

A Unicode bidirectional control (an embedding, override, isolate, or a scopeless mark)
appears outside a string literal or comment, or opens inside one and reaches the end of that
token unclosed.

```quilon ignore
x = 1 ‮ + 2
```

Keep bidirectional controls inside string literals and comments, and close every opener
before the literal or comment ends.

## Parser

### Q005 — unexpected token

The parser found one token where the grammar requires another — a closing `)`, a name, a
type, a pattern, an expression.

```quilon ignore
^ = () -> Num => < (1 + 2 >
```

The message names both the token found and the one required; supply the required one:
`^ = () -> Num => < (1 + 2) >`.

### Q006 — expression nesting too deep

An expression nests more than 128 levels of parentheses, literals, blocks, element types,
constructor patterns, or prefix operators.

```quilon ignore
^ = () -> Num => < ((((((… 200 levels …)))))) >
```

Split the expression into named bindings.

### Q007 — too many parameters

A function, method, or lambda declares more than 10 parameters.

```quilon ignore
f = (a :: Num, b :: Num, c :: Num, d :: Num, e :: Num, f :: Num, g :: Num, h :: Num, i :: Num, j :: Num, k :: Num) -> Num => < a >
```

Group the parameters into a record type and take that record as one parameter.

### Q008 — match with no arms

A `?` match has no `|` arm.

```quilon ignore
^ = () -> Num => < 1 ? >
```

Write at least one arm: `1 ? | 1 => 0 | _ => 1`.

### Q009 — interpolation hole with more than one expression

A backtick hole inside a string holds more than one expression.

```quilon ignore
^ = () -> Num => < "`1 2`".size >
```

Put one expression in the hole: `` "`1 + 2`" ``.

### Q010 — import path with interpolation

A `<< "…"` import path contains a backtick hole.

```quilon ignore
<< "lib/`name`.qn"
```

Write the path as a plain literal: `<< "lib/util.qn"`.

### Q011 — qualified name through a missing import

A top-level line reaches into a module (`name.member`) with no `<<` for that module above
it. The common case is a test suite missing its harness.

```quilon ignore
test.describe("math", () => < test.it("adds", () => expect(1 + 1, equals(2))) >)
```

Add the import above the line: `<< core.test`.

### Q012 — ambiguous `{ }` type declaration

A `Name = { … }` holds only method-shaped members, so it reads both as a type declaration
and as a record literal.

```quilon ignore
Counter = { bump = => 1 }
```

Add a `::` field to declare a type (`Counter = { n :: Num, bump = => it.n + 1 }`), or give
the members plain values to write a record literal (`counter = { bump = 1 }`).

### Q013 — operator member declared with `:=`

An operator member, or the render member `` ` ``, is declared with `:=`. An operator
yields a value and leaves `it` as it is.

```quilon ignore
Point = { x :: Num, + := (other :: Point) -> Point => < Point { x = it.x + other.x } > }
```

Declare it with `=`: `+ = (other :: Point) -> Point => < … >`.

### Q014 — lowercase sum-type variant

A variant of a sum type starts with a lowercase letter.

```quilon ignore
Color = red / Green
```

Capitalize every variant: `Color = Red / Green`.

### Q015 — sum type with fields or a mutating method

A sum type's trailing `{ }` block declares a `::` field, or a `:=` method. A sum carries
data in its variant payloads and its block holds methods only.

```quilon ignore
Shape = Circle(Num) / Square(Num) { name :: Text }
```

Move the data into a payload (`Circle(Num, Text)`) and keep the block to `=` methods.

### Q016 — bare expression as a function body

A function or method body is a bare expression after `=>`. A lambda's body may be bare;
a declaration's is a block.

```quilon ignore
double = (x :: Num) -> Num => x * 2
```

Write the body as a block: `double = (x :: Num) -> Num => < x * 2 >`.

### Q017 — `>>` where two block closers were meant

Two block closers written together (`>>`) lex as the export marker.

```quilon ignore
^ = () -> Num => < f = () -> Num => < 1 >>
```

Separate them with a space: `< 1 > >`.

## Imports

### Q018 — `@` primitive declared outside the corelib

A user source declares a name starting with `@`. The `@` marks a built-in IO primitive,
which only the corelib defines; user code calls one.

```quilon ignore
@sleep = (seconds :: Num) -> $ => < $ >
```

Declare an ordinary function, and call the corelib's primitive where a primitive is meant.

### Q019 — missing module

A `<<` import names a built-in module the compiler lacks, a file that is missing or
unreadable, or `core.text`, which the compiler merges on its own.

```quilon ignore
<< core.magic
```

Import one of the built-in modules (`core.io`, `core.test`, `core.cli`, `core.time`,
`core.net`, `core.http`, `core.info`) or an existing `.qn` file by path.

### Q020 — private member reached through its module

A qualified reference names a member the module keeps private, or one it lacks.

```quilon ignore
<< core.io
^ = () -> Num => < io.secret() >
```

Reach an exported member, or mark the member `>>` in its module.

### Q021 — name claimed by an import

A binding, parameter, or pattern name is the short name an import binds (`io` for
`<< core.io`).

```quilon ignore
<< core.io
io = 1
```

Rename the binding; the import keeps its name.

### Q022 — import cycle

A module imports itself, directly or through other modules.

```quilon ignore
<< "a.qn"   ~ where a.qn imports this file
```

Move the shared definitions into a third module both import.

### Q023 — two modules with one name

Two imported modules bind the same short name — two files named `util.qn` in different
directories, or a file stem that is unusable as a binding.

```quilon ignore
<< "lib/util.qn"
<< "vendor/util.qn"
```

Rename one of the files, or drop one of the imports.

### Q024 — module binding used as a value

An import's binding name appears where a value is required.

```quilon ignore
<< core.io
^ = () -> Num => < x = io  0 >
```

Reach the module's exports through the binding: `io.print(…)`.

### Q025 — ambiguous module prefix

A short prefix is bound by more than one imported module.

```quilon ignore
<< core.http
<< "vendor/http.qn"
^ = () -> Num => < http.send(request)  0 >
```

Write the full path: `core.http.send(…)`.

### Q026 — test blocks without `core.test`

A file has top-level `test.describe` blocks and `quilon test` finds the harness's summary
function out of scope.

```quilon ignore
test.describe("math", () => < test.it("adds", () => expect(2, equals(2))) >)
```

Add `<< core.test` above the first block.

## Checker

### Q027 — undefined name

A name is used with no definition above it. Names resolve top to bottom.

```quilon ignore
^ = () -> Num => < total >
```

Define the name before its use: `total = 1` on a line above.

### Q028 — type mismatch

An expression has one type where another is required.

```quilon ignore
x :: Num = "seven"
```

Give the binding a value of the annotated type, or change the annotation to match.

### Q029 — call on a data value

A value of a data type is called with `( )`.

```quilon ignore
^ = () -> Num => < n = 1  n(2) >
```

Call a function, or index a collection with `[ ]`.

### Q030 — wrong number of arguments

A call passes more or fewer arguments than the function declares.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
^ = () -> Num => < double(1, 2) >
```

Pass exactly the declared parameters: `double(1)`.

### Q031 — assignment to an immutable binding

A binding made with `=` is reassigned.

```quilon ignore
^ = () -> Num => < n = 1  n := 2  n >
```

Bind it with `:=` to allow writes: `n := 1`.

### Q032 — field write through an immutable binding

A field is written through a binding made with `=`.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p = Point { x = 1 }  p.x := 2  p.x >
```

Bind the record with `:=`: `p := Point { x = 1 }`.

### Q033 — `:=` binding aliasing an immutable value

A `:=` binding takes the value of an `=` binding, a parameter, or the receiver `it` of an
`=` method. A value bound with `=` stays immutable through every alias.

```quilon ignore
^ = () -> Num => < a = [1]  b := a  0 >
```

Bind with `=`, or build a fresh value: `b := a + []`.

### Q034 — `=` binding aliasing a mutable value

An `=` binding takes the value of a `:=` binding; writes through the mutable binding would
change the `=`-bound value.

```quilon ignore
^ = () -> Num => < a := [1]  b = a  0 >
```

Bind with `:=`, or build a fresh value.

### Q035 — mutating method on an immutable receiver

A `:=` method is called on a receiver bound with `=`.

```quilon ignore
Counter = { n :: Num, bump := () -> $ => < it.n := it.n + 1 > }
^ = () -> Num => < c = Counter { n = 0 }  c.bump()  c.n >
```

Bind the receiver with `:=`: `c := Counter { n = 0 }`.

### Q036 — mutating method declared with `=`

A method declared with `=` writes to `it`.

```quilon ignore
Counter = { n :: Num, bump = () -> $ => < it.n := it.n + 1 > }
```

Declare it with `:=`: `bump := () -> $ => < … >`. A lambda parameter named `it` inside the
body shadows the receiver; rename it where the write targets the lambda's own value.

### Q037 — duplicate definition

A name is defined twice in one scope, and the two definitions form no overload set.

```quilon ignore
x = 1
x = 2
```

Give each definition its own name.

### Q038 — no matching overload

A call, or an operator, has argument types that match no member of its overload set.
Dispatch is by exact type.

```quilon ignore
^ = () -> Num => < 1 + "x"  0 >
```

Pass the types a member takes; the `help:` line lists the members. To join a number and
text, interpolate: `` "`n`x" ``.

### Q039 — ambiguous overload

More than one member of an overload set matches the argument types — two members share a
parameter list.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (n :: Num) -> Num => < n * 2 >
```

Give the members distinct parameter types.

### Q040 — overload member with an unannotated parameter

A member of an overload set leaves a parameter without a type. Exact dispatch reads every
member's full signature.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (t) -> Num => < t.size >
```

Annotate every parameter: `f = (t :: Text) -> Num => < t.size >`.

### Q041 — parameter without a type

A function parameter has no type annotation and no context to take one from.

```quilon ignore
double = (x) => < x * 2 >
```

Annotate the parameter: `double = (x :: Num) => < x * 2 >`.

### Q042 — parameter count differs from the function type

A definition, or a lambda, declares a different number of parameters than the function type
it must match.

```quilon ignore
f :: (Num, Num) -> Num = (a :: Num) => a
```

Declare exactly the parameters the type states.

### Q043 — lambda parameter with an open type

A lambda leaves a parameter unannotated in a position that states no function type — or
an overload set the other arguments leave open.

```quilon ignore
^ = () -> Num => < g = (x) => x + 1  g(1) >
```

Annotate the parameter: `(x :: Num) => x + 1`.

### Q044 — recursive function without a return type

A function calls itself and declares no `-> T`. A call to the function needs its return
type before the body is checked.

```quilon ignore
fact = (n :: Num) => < n <= 1 ? 1 : n * fact(n - 1) >
```

Annotate the return type: `fact = (n :: Num) -> Num => < … >`.

### Q045 — write to a `Site` field

A field of a `Site` is assigned. A location is a value.

```quilon ignore
f = (site :: Site) -> $ => < site.line := 1 >
```

Read the fields; build a fresh record where a different location is wanted.

### Q046 — misplaced `Site` parameter

A `Site` parameter sits anywhere but last in a top-level function, or on a lambda or
method. The compiler fills a call site only as the last parameter of a top-level function.

```quilon ignore
check = (site :: Site, n :: Num) -> $ => < $ >
```

Move the `Site` parameter last: `check = (n :: Num, site :: Site) -> $ => < $ >`.

### Q047 — call before the definition

A call reaches an overload set whose every member is defined below the call.

```quilon ignore
^ = () -> Num => < f(1) >
f = (n :: Num) -> Num => < n >
```

Move the definition above the call.

### Q048 — overload member without a return type

A member of an overload set omits its `-> T`. Exact dispatch reads the full signature.

```quilon ignore
f = (n :: Num) => < n >
f = (t :: Text) -> Num => < t.size >
```

Annotate the return type: `f = (n :: Num) -> Num => < n >`.

### Q049 — comparison operator returning a type other than `Bool`

A member for `==`, `!=`, `<`, `<=`, `>`, or `>=` declares a return type other than `Bool`.

```quilon ignore
Point = { x :: Num, == = (other :: Point) -> Num => < 0 > }
```

Return `Bool`: `== = (other :: Point) -> Bool => < it.x == other.x >`.

### Q050 — nested pattern inside a constructor pattern

A constructor pattern's argument is a literal or another constructor. The match dispatches
on the constructor tag alone.

```quilon ignore
^ = () -> Num => < r = Ok(1)  r ? | Ok(1) => 1 | _ => 0 >
```

Bind the payload and compare it in the arm: `| Ok(n) => n == 1 ? 1 : 0`.

### Q051 — non-exhaustive match

A `?` match leaves values uncovered — a sum type with variants no arm lists, or any other
type with no `_` arm.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Red => 1 >
```

Add the missing arms, or a `_` arm.

### Q052 — unknown variant

A constructor pattern names a variant the sum type lacks.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Blue => 1 | _ => 0 >
```

Name one of the type's variants; the message lists them.

### Q053 — constructor pattern on a non-sum value

A constructor pattern matches a value of a type with no variants, or one whose type is
still open.

```quilon ignore
^ = () -> Num => < 5 ? | Ok(x) => x | _ => 0 >
```

Match the value itself — a literal, a binding, or `_` — or annotate the value's type.

### Q054 — unsupported `^` signature

The entry point declares parameters other than the supported forms.

```quilon ignore
^ = (n :: Num) -> Num => < n >
```

Declare `^` as `()`, `(args :: []Text)`, or `(args :: []Text, env :: [|Text => Text|])`.

### Q055 — invalid argument to a built-in

A built-in method receives an argument outside its contract that is visible statically —
`Text.repeat` with a negative literal count, an index into a `Map`, an empty `< >` block
where a value is required.

```quilon ignore
^ = () -> Num => < "ab".repeat(-1).size >
```

The message states the contract; pass an argument that meets it.

### Q056 — top-level binding that has to be computed

A top-level `=` binding holds a call, an operator, an array, a record, or `Text`. A
top-level binding becomes a global whose initializer is a constant.

```quilon ignore
total = 1 + 2
^ = () -> Num => < total >
```

Move the computation into `^` or the function that uses it; keep a top-level binding to a
`Num`, `Bool`, or `$` literal, or a function.

### Q057 — operator defined at the top level

An operator symbol names a top-level definition. An operator is a member of the record or
sum type it operates on.

```quilon ignore
+ = (a :: Point, b :: Point) -> Point => < Point { x = a.x + b.x } >
```

Define it inside the type's `{ }`, where `it` is the left operand:
`+ = (other :: Point) -> Point => < … >`.

### Q058 — operator member with the wrong parameter count

An operator member declares more or fewer than one explicit parameter. `it` is the left
operand; the one parameter is the right.

```quilon ignore
Point = { x :: Num, + = (a :: Point, b :: Point) -> Point => < a > }
```

Declare one parameter: `+ = (other :: Point) -> Point => < … >`.

### Q059 — assertion without a matcher

`assert` or `expect` is called with anything but a value and one of the matchers.

```quilon ignore
^ = () -> $ => < assert(1 == 1) >
```

Pass the value and a matcher: `assert(1, equals(1))`. The matchers are `equals`,
`contains`, `not`, `isOk`, `isNotOk`.

### Q060 — `expect` outside a test case

`expect` is called outside an `it` case of a `describe` block.

```quilon ignore
^ = () -> $ => < expect(1, equals(1)) >
```

Use `assert`, which reports and exits, outside a test case.

### Q061 — matcher with the wrong argument count

A matcher is called with more or fewer arguments than it takes.

```quilon ignore
^ = () -> $ => < assert(1, equals(1, 2)) >
```

Pass the matcher's arguments: `equals(1)`.

### Q062 — matcher on a type outside its reach

A matcher meets a type it has no way to inspect: `equals` on a type with no `==`,
`contains` on anything but `Text` or an array, `isOk`/`isNotOk` on anything but a `Result`.

```quilon ignore
^ = () -> $ => < assert(1, contains(1)) >
```

Apply a matcher that reads the type: `assert([1], contains(1))`.

### Q063 — unknown member

`value.name` names a member the value's type lacks. A function of the same name in scope
answers the plain call form only.

```quilon ignore
<< core.io
^ = () -> Num => < n = 1  n.print()  0 >
```

Call the function on the value: `io.print(n)`.

### Q064 — method called as a function

`name(value)` names a member of the value's type, and the top level has no function of that
name.

```quilon ignore
Counter = { n :: Num, bumped = () -> Num => < it.n + 1 > }
^ = () -> Num => < c = Counter { n = 1 }  bumped(c) >
```

Call the member on the value: `c.bumped()`.

### Q065 — value with no rendering

`io.print`, `io.eprint`, or `io.write` receives a value with no `` ` `` render member — a
function.

```quilon ignore
<< core.io
f = () -> Num => < 1 >
^ = () -> Num => < io.print(f)  0 >
```

Print a renderable value, or give the type a `` ` `` member.

### Q066 — no `^` entry point

`run`, `build`, or `compile` is given a file with no `^` function.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
```

Define the entry point: `^ = () -> Num => < 0 >`.

## Code generation and build

### Q067 — code generation failed

The checked program reached a case the code generator lacks. The message names the case.

```text
error[Q067]: unknown array method `frobnicate`
```

File the message and the program as an issue; the checker and the generator agree on every
construct the language reference documents.

### Q068 — native build failed

The linker is missing, or the link or the write of the output failed. The message carries
the linker's own words.

```text
error[Q068]: linker `clang` not found on PATH. Install it, or pass `--linker <name>` (e.g. `--linker gcc`).
```

Install `clang`, or pass `--linker gcc`.

## Runtime

### Q069 — assertion failed

An `assert` or `expect` found its value outside what the matcher accepts. `assert` exits
101; `expect` marks the running case failed and continues.

```quilon ignore
^ = () -> $ => < assert(2 + 2, equals(5)) >
```

The message states what was expected and what was found; fix the program or the
expectation.

### Q070 — index out of bounds

An `array[i]` read has `i` below 0, at or past the array's size, or a fraction.

```quilon ignore
^ = () -> Num => < a = [1, 2]  a[5] >
```

Index within `0` and `a.size - 1`, or check `i < a.size` first.

### Q071 — fractional or unrepresentable range endpoint

A computed `lo <- hi` endpoint is a fraction, NaN, infinite, or beyond the whole numbers a
`Num` holds exactly.

```quilon ignore
^ = () -> Num => < half = 0.5  (1 <- half).size >
```

Compute whole-number endpoints.

### Q072 — no arm matched

A `?` match reached the end of its arms. The checker proves every match exhaustive; this is
the runtime's backstop.

```text
error[Q072]: no arm of this match matched the value
```

File the program as an issue.

### Q073 — allocation failed

The collector had no memory for the request, or the requested size was outside what a
`Num` represents.

```text
error[Q073]: out of memory allocating 17179869184 bytes
```

Allocate within the machine's memory, and compute sizes that stay whole numbers.

### Q074 — reading stdin failed

`@readStdin` met an IO error on stdin — one other than end of file.

```text
error[Q074]: @readStdin failed: Input/output error (os error 5)
```

Run the program with a readable stdin.
