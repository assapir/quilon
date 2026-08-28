---
title: "Records"
sidebar:
  order: 2
---

# Records
Anonymous structs with named fields:
```quilon
user = { name = "Alice", age = 30 }
n    = user.name
```
Fields may hold any type — `Text`, arrays, nested arrays, etc. — and read back at
their real type (no numeric-only restriction). (See `examples/records.qn` and
`examples/composites.qn`, which exercises a `Text` record field, an array of `Text`,
and a nested array together.)

## Named record types with methods
Methods take an implicit `it` (the receiver):
```quilon
User = {
  name :: Text,
  age  :: Num,
  greet   = => "Hello, " + it.name,
  olderBy = years => it.age + years
}

u = User { name = "Alice", age = 30 }
g = u.greet()          ~ "Hello, Alice"
a = u.olderBy(5)       ~ 35
```
(See `examples/methods.qn`.)

A method declared with `:=` instead of `=` is a **setter** — it may mutate its
receiver — and calling one requires a mutable (`:=`) receiver
(see [Mutation](../mutation.md)).

An unannotated method parameter defaults to `Num` (as in any [ordinary
definition](../functions/overloading.md)), and call sites are held to that default:
`t.add("hi")` on `add = (x) => it.v + x` is a type error, not a runtime surprise.

A method answers the plain form `name(recv, args)` too — and `recv |> name(args)`, which
[is](../expressions/pipe.md) that call. Only the `.` form refuses the top-level fallback:
`recv.name(...)` its type cannot answer is an error, where `name(recv, ...)` goes on to
look the name up as usual.
```quilon ignore
(5).double()   ~ error: 'Num' has no member 'double'
double(5)      ~ 10
5 |> double()  ~ 10
```
This holds for the names the compiler provides too: a record prints as `print(c)`, never
`c.print()`.
