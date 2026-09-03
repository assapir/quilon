---
title: "Expressions"
sidebar:
  order: 0
---

# Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](../functions/overloading.md): `Num + Num` adds, `Text + Text` concatenates, and on arrays it concatenates / appends / prepends (`[]T + []T`, `[]T + T`, `T + []T`, all yielding a new `[]T` — see [Array concatenation](../collections/arrays.md#array-concatenation--)). `%` is the f64 remainder, defined on fractional operands too (`7.5 % 2` → `1.5`); the result takes the **dividend's** sign (`-7 % 3` → `-1`, `7 % -3` → `1`).
- **Comparison:** `== != < <= > >=`; all return `Bool`. Equality (`==`/`!=`) is over `Num`, `Text` and `Bool`; ordering (`< <= > >=`) is over `Num` and (lexicographically) `Text`. Each is a [user-overloadable operator](../functions/overloading.md#operator-overloading). Comparing two different types is a no-matching-overload error.
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` and `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is less-than. A `>` **closes a block by default**; it is
> **greater-than when an operand follows it on the same line** — an identifier, a literal,
> `(`, `[`, `{`, or a prefix `-`/`!`. `a > b`, `f(x > y)`, `a > -b` and `"b" > "a"` are
> comparisons; a `>` before a `)`, `]`, `}`, `,`, a `~` comment, or the end of the line
> closes its block. A block-bodied lambda sits inside a call on one line:
> ```quilon ignore
> xs.each(x => <
>   total := total + x
> >)
> ```
> The right operand of a comparison `>` is on the same line as the `>`. Two adjacent
> closers are separated by a space — `> >` — `>>` being the export marker. `<=`/`>=`/`>>`
> are distinct tokens.

> **Statement boundaries — line-first `(` / `[` / `{`.** A newline ends a statement. The
> grammar reads across newlines except for two rules: the `>` rule above, and this one —
> a `(`, `[`, or `{` that is the **first token on its line** begins a new statement.
> A call, index, or constructor opens on the **same line** as the expression it applies
> to; once opened it may span lines. A continuation line may start with `.` or an
> operator.
> ```quilon ignore
> ~ (statements inside a `< >` block / `^` body)
> ~ each of these continues the expression:
> sum = add(40,
>   2)                                  ~ `(` opened on add's line; args may span lines
> total = nums.map(n => n * 2)
>   .reduce(0, (acc, n) => acc + n)     ~ `.`-led line chains
> p = Point {
>   x = 1, y = 2 }                      ~ `{` opened on Point's line; body may span lines
>
> ~ a line-first `(`, `[`, or `{` is a NEW statement:
> x = f()
> (1 + 2)                               ~ a statement of its own, separate from `f()`
> b = a
> [3, 4].each(n => io.print(n))            ~ a statement of its own, separate from `a`
> e = origin
> { x = 9, y = 9 }                      ~ a statement of its own, separate from `origin`
>
> ~ a call opens its argument list on the callee's line:
> x = f
> (10)                                  ~ `(10)` is a new statement; `f` is bound as a value
> ```
> (See `examples/statements.qn`.)
- **Ternary:** `cond ? then : else`.
- **Blocks:** `< stmt… last >` evaluate to their last statement — an expression's own
  type, or `$` (Unit) when the last statement is a declaration (`=`/`:=`). A block goes in
  **body** position — a function's, a lambda's, or a method's; operand positions take
  expressions. A function's and a method's body is **always** a block, for a single
  expression too (`double = (x :: Num) => < x * 2 >`); a **lambda** may write a bare
  expression (`xs.map(x => x * 2)`). An **empty** block — no statements at all — is a
  compile error.
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

`1 <- 2 + 2` is `1 <- 4`, and `1 < 2 == true` is `(1 < 2) == true`. Parentheses settle
every other grouping. `>` appears in the table in its operator reading; the lexer settles
first whether a given `>` takes that reading — see the [`>` rule](#expressions).
