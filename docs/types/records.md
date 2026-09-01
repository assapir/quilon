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
  greet   = => < "Hello, " + it.name >,
  olderBy = years => < it.age + years >
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

A method is reached through `recv.name(...)` and nowhere else, and a top-level function
through `name(args)` and nowhere else — neither answers for the other. `recv.name(...)`
looks for `name` on `recv`'s type alone; `name(recv, args)` looks in the top-level
namespace alone.
```quilon ignore
Counter = { value :: Num, bump = (by :: Num) -> Num => < it.value + by >}
double = (x :: Num) -> Num => < x * 2 >

c.bump(5)      ~ 35
bump(c, 5)     ~ error: no function 'bump' in scope
(5).double()   ~ error: 'Num' has no member 'double'
double(5)      ~ 10
```
The same holds for the methods reserved on the built-in types: `"a,b".split(",")` reaches
`Text`'s `split`, and `split("a,b", ",")` reaches nothing.
