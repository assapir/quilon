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

## A member call resolves against the receiver's type

`recv.name(...)` asks `recv`'s type for `name` — a method it declares, or a built-in
method reserved on `Text`, arrays, `Map` and `Set`. It never looks in the top-level
namespace, so a name there — a function of your own, or one the compiler provides like
[`print`](../corelib/io.md) — is a different thing and cannot take the call over:
```quilon
Counter = { value :: Num, bump = (by :: Num) -> Num => it.value + by }
bump = (by :: Num) -> Num => by * 100

^ = () -> Num => <
  c :: Counter = Counter { value = 30 }
  c.bump(5) + bump(5)     ~ 35 + 500: the method, then the function
>
```
A name the receiver's type does not have is an error naming both, even when a top-level
function of that name is in scope:
```quilon ignore
(5).double()   ~ error: 'Num' has no member 'double'
```
So a type is printable through `print(c)`, never `c.print()` — printing renders the value
through the type's `` ` `` member, and `print` itself is not a member of anything.

A method also answers the plain form `name(recv, args)` — and `recv |> name(args)`, which
[is](../expressions/pipe.md) that call. What only the `.` form does is refuse the top-level
fallback: `recv.name(...)` its type cannot answer is the error above, where `name(recv, ...)`
goes on to look the name up as usual.
