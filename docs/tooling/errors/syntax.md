---
title: "Syntax errors"
sidebar:
  order: 1
---

# Syntax errors

QN0xx and QN1xx: the lexer's and parser's own errors, raised reading source before any name
is resolved.

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

A `"` opens a string that reaches the end of its line with no closing `"`.

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

### QN005 — scientific notation literal

A `Num` literal is glued to `e` or `E`, an optional `+`/`-`, and digits — the exponent
notation this grammar's literal syntax stops short of.

```quilon ignore
price = 1.5e-3
```

Write the value in plain decimal: `price = 0.0015`.

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
[the disambiguation rule](../../types/sum-types.md) — so this fires from the third variant on;
a lowercase first or second variant instead reads as dividing undefined names, an
[undefined name](semantics.md#qn300--undefined-name) error.)

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

### QN113 — disallowed character glued to a name

A definition or parameter name is immediately followed, with no space, by a character
that is not part of a name — a name holds only letters, digits and `_`.

```quilon ignore
isEmpty? = () -> Bool => < true >
```

Drop the character, or fold a `-`-joined continuation into the name: `isEmpty` (not
`isEmpty?`), `myCount` (not `my-count`). The `help:` line names the fix for the exact name
at hand.

### QN114 — match used as a match-arm body without parentheses

A match arm's body is itself a match, written bare. The inner match's `|` arm loop reads
past its own arms and keeps consuming the outer match's, since both use `|` and nothing
marks where the inner one ends.

```quilon ignore
Snack = Popcorn(Num) / Pretzel(Num)
craving = (choice :: Snack, size :: Num) -> Num => < choice ?
  | Popcorn(n) => size ? | 0 => n | _ => n * 2
  | Pretzel(n) => n
>
```

Parenthesize the nested match: `| Popcorn(n) => (size ? | 0 => n | _ => n * 2)`. A match
nested inside a block, a call argument, or a ternary branch is already delimited and needs
no parentheses of its own.

### QN115 — a line-final `>` closed a block earlier than intended

A `>` closes its block when it ends its line — the rule that also settles when it reads
as the greater-than operator ([the `>` box](../../expressions/README.md#expressions)).

```quilon ignore
mangoTally = (crate :: Num) -> Num => <
  ripe = 5
  bruised = 3
  ripe >
  bruised
>
```

`ripe >` ends its line, so it closes `mangoTally`'s block right there. The report points
at that `>` — the actual cause — wherever the parse derails further down. To compare, put
the right operand on the same line as `>`: `ripe > bruised`.

### QN116 — text pattern with interpolation

A `Text` pattern (`| "…" =>`) contains a backtick hole. A pattern is checked against a
fixed literal, so its text cannot depend on a runtime value.

```quilon ignore
greet = (title :: Text, name :: Text) -> Text => < name ?
  | "`title` Smith" => "the whole family"
  | _                => "just " + name
>
```

Write the pattern as a plain literal — `| "Smith" => …` — and compare the computed part
in the arm's body instead.
