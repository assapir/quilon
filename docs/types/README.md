---
title: "Types"
sidebar:
  order: 0
---

# Types

## `Num`
All numbers — integers and floats are one unified type: an IEEE-754 double (64-bit float).
```quilon
x = 42
y = 3.14
z = x + y          ~ mixed arithmetic
```

### The exact-integer limit
Being a double, a `Num` represents every whole number exactly only up to **2^53**
(`9007199254740992`). Past that, consecutive integers collide — 2^53 + 1 is not
representable, so it reads back as 2^53 — and a whole number there is no longer a distinct
value. Arithmetic still works, but treat 2^53 as the range over which integer results are
trustworthy. Where a whole number has to be exact, the compiler enforces the limit: a
[range endpoint](../expressions/ranges-and-spread.md#endpoints-must-be-whole-numbers) past
it is an error rather than a silently wrong count.

## `Bool`
`true` / `false` (the literals are lowercase; note that a `Bool` *renders* as capitalized
`True`/`False` — see [interpolation](text.md#string-interpolation-and-the-render-operator-)).

## `Unit` — `$`
The **unit type**, written `$`. It has exactly one value, also written `$` — so `$` is
both the type (in type position, e.g. `-> $`) and its sole value (in value position),
analogous to `()` in Rust/ML. Use it for side-effecting expressions and functions whose
result is meaningless. `print` and `eprint` return `$`. `$` is compatible only with `$`.
```quilon
<< core.io
log = (m :: Text) -> $ => < io.print(m) > ~ a function whose result is meaningless
^ = () -> $ => < log("started") >    ~ a `$` body exits 0 (it is not a Num)
```

Arrays (`[]T`) live with the other built-in parametric collections — see
[`collections/arrays.md`](../collections/arrays.md).
