---
title: "Types"
sidebar:
  order: 0
---

# Types

The built-in type names — `Num`, `Bool`, `Text`, `Result`, `Site`, `Map`, `Set` — are
[reserved](../tooling/errors.md#qn344--reserved-name): a program names them in annotations
and declares its own types under other names.

## `Num`
All numbers — integers and floats are one unified type: an IEEE-754 double (64-bit float).
```quilon
x = 42
y = 3.14
z = x + y          ~ mixed arithmetic
```

### The exact-integer limit
A `Num` is a double, and represents every whole number exactly up to **2^53**
(`9007199254740992`). Past that, consecutive integers collide — 2^53 + 1 reads back as
2^53. Integer results are exact within 2^53. Where a whole number has to be exact, the
compiler enforces the limit: a
[range endpoint](../expressions/ranges-and-spread.md#endpoints-are-whole-numbers) past
it is an error.

## `Bool`
`true` / `false` — lowercase literals. A `Bool` *renders* as capitalized `True`/`False` —
see [interpolation](text.md#string-interpolation-and-the-render-operator-).

## `Text`
UTF-8 text, built in like `Num` and `Bool`: the type and its methods are available without
an import. A `Text` is a **sequence of graphemes** (user-perceived characters): every
index and length counts grapheme clusters, `.at(i)` reads one (itself a length-1
`Text`), `.graphemes()` yields them all as a `[]Text`, and `+` concatenates. The
primitive methods are native segmentation; the rest are ordinary Quilon the compiler
merges in. The full reference, methods table, and string interpolation live in
[`text.md`](text.md).
```quilon
"héllo".length       ~ 5 graphemes (é is one, whatever its bytes)
"a🌍b".at(1)         ~ Ok("🌍") — the whole cluster
"a,b".split(",")     ~ ["a", "b"]
```

## `Unit` — `$`
The **unit type**, written `$`. It has exactly one value, also written `$`: `$` is both
the type (in type position, e.g. `-> $`) and its sole value (in value position). It is
the result type of a side-effecting function. `print` and `eprint` return `$`. `$` is
compatible with `$` alone.
```quilon
<< core.io
log = (m :: Text) -> $ => < io.print(m) > ~ a function with a unit result
^ = () -> $ => < log("started") >    ~ a `$` body exits 0
```

Arrays (`[]T`) live with the other built-in parametric collections — see
[`collections/arrays.md`](../collections/arrays.md).
