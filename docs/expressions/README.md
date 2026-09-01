---
title: "Expressions"
sidebar:
  order: 0
---

# Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](../functions/overloading.md): `Num + Num` adds, `Text + Text` concatenates, and on arrays it concatenates / appends / prepends (`[]T + []T`, `[]T + T`, `T + []T`, all yielding a new `[]T` — see [Array concatenation](../collections/arrays.md#array-concatenation--)). `%` is the f64 remainder and works on fractional operands too (`7.5 % 2` → `1.5`); the result takes the **dividend's** sign (`-7 % 3` → `-1`, `7 % -3` → `1`), like C `fmod` / Rust `%`.
- **Comparison:** `== != < <= > >=`; all return `Bool`. Equality (`==`/`!=`) is over `Num`, `Text` and `Bool`; ordering (`< <= > >=`) is over `Num` and (lexicographically) `Text`. Each is a [user-overloadable operator](../functions/overloading.md#operator-overloading), and comparing two different types is a no-matching-overload error — there is no coercion.
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` vs. `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is always less-than (a block can't start mid-expression). A `>`
> **closes a block by default**; it is **greater-than only when an operand follows it on
> the same line** — an identifier, a literal, `(`, `[`, `{`, or a prefix `-`/`!`. So `a > b`,
> `f(x > y)`, `a > -b` and `"b" > "a"` are comparisons, while a `>` before a `)`, `]`, `}`,
> `,`, a `~` comment, or the end of the line closes its block — which is what lets a
> block-bodied lambda sit inside a call on one line:
> ```quilon ignore
> xs.each(x => <
>   total := total + x
> >)
> ```
> Two rules follow: don't end a line with a comparison `>` (the right operand must be on
> that line), and separate two adjacent closers with a space — `> >`, since `>>` is the
> export marker. `<=`/`>=`/`>>` are distinct tokens and unaffected.

> **Statement boundaries — line-first `(` / `[` / `{`.** Quilon has no statement separator.
> The grammar is newline-insensitive but for two rules: the `>` rule above, and this
> one — a `(`, `[`, or `{` that is the **first token on its line** begins a new statement
> rather than continuing the previous expression as a call, index, or constructor.
> A call, index, or constructor must open on the **same line** as the expression it applies
> to; once opened it may span lines. A continuation line may still start with `.` or
> an operator.
> ```quilon ignore
> ~ (statements inside a `< >` block / `^` body)
> ~ OK — these all continue the expression:
> sum = add(40,
>   2)                                  ~ `(` opened on add's line; args may span lines
> total = nums.map(n => n * 2)
>   .reduce(0, (acc, n) => acc + n)     ~ `.`-led line chains
> p = Point {
>   x = 1, y = 2 }                      ~ `{` opened on Point's line; body may span lines
>
> ~ OK — a line-first `(`, `[`, or `{` is a NEW statement:
> x = f()
> (1 + 2)                               ~ not the call `f()(1 + 2)`
> b = a
> [3, 4].each(n => io.print(n))            ~ not the index `a[3, 4]`
> e = origin
> { x = 9, y = 9 }                      ~ not the constructor `origin { x = 9, y = 9 }`
>
> ~ DON'T — a call may not open its argument list on the next line:
> x = f
> (10)                                  ~ NOT the call `f(10)`: `(10)` is a new statement
> ```
> (See `examples/statements.qn`.)
- **Ternary:** `cond ? then : else`.
- **Blocks:** `< stmt… last >` evaluate to their last expression. A block goes in **body**
  position — a function's, a lambda's, or a method's — not in operand position, so a block
  is never the left or right side of an operator:
```quilon
total = () -> Num => <
  x = 10
  y = 20
  x + y          ~ total() is 30
>
```

## Operator precedence
Least-priority level first; every level is **left-associative** except `<-`, which is
non-associative (`1 <- 2 <- 3` is a parse error).

| | Operators |
|---|---|
| less priority | `:=` (reassignment) |
| | `? :` ternary · `?` `\|` match |
| | `\|\|` |
| | `&&` |
| | `==` `!=` |
| | `<` `<=` `>` `>=` |
| | `<-` (range) |
| | `+` `-` |
| | `*` `/` `%` `+-` |
| | `-x` `!x` (prefix) |
| more priority | `.field` · `.method(…)` · `f(…)` · `xs[i]` |

So `1 <- 2 + 2` is `1 <- 4`, and `1 < 2 == true` is
`(1 < 2) == true`. Parenthesize anything else. `>` appears in the table in its operator
reading; whether a given `>` gets that reading at all is settled first, in the lexer — see
the [`>` rule](#expressions).
