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
Fields may hold any type — `Text`, arrays, nested arrays — and read back at their
declared type. (See `examples/records.qn` and
`examples/composites.qn`, which exercises a `Text` record field, an array of `Text`,
and a nested array together.)

## Named record types with methods
Methods take an implicit `it` (the receiver):
```quilon
User = {
  name :: Text,
  age  :: Num,
  greet   = => < "Hello, " + it.name >,
  olderBy = (years :: Num) => < it.age + years >
}

u = User { name = "Alice", age = 30 }
g = u.greet()          ~ "Hello, Alice"
a = u.olderBy(5)       ~ 35
```
(See `examples/methods.qn`, which also exercises a method with parameters as the first
member.) Members may appear in any order — a method may come before the fields it uses.

### Type declaration vs. record literal
A block containing a `::` field declaration declares a type. A block of `name = value`
assignments is a record literal. A block containing only method definitions
(`name = => …`, `name = (params) -> R => …`) with no `::` field is a **compile error**:
add a `::` field to declare a type, or use plain values to write a record literal.

A method declared with `:=` is a **setter** — it may mutate its receiver — and calling one
requires a mutable (`:=`) receiver. Records have reference
semantics, and the binding operator governs the value itself: an `=`-bound record is
immutable through every alias
(see [Mutation](../mutation.md), including [deep
immutability](../mutation.md#deep-immutability)).

A method parameter is annotated, like an [ordinary definition](../functions/overloading.md)'s:
`add = (x) => it.v + x` is a compile error naming the unannotated `x`.

A method is reached through `recv.name(...)`, and a top-level function through
`name(args)`. `recv.name(...)` looks for `name` on `recv`'s type; `name(recv, args)` looks
in the top-level namespace.
```quilon ignore
Counter = { value :: Num, bump = (by :: Num) -> Num => < it.value + by >}
double = (x :: Num) -> Num => < x * 2 >

c.bump(5)      ~ 35
bump(c, 5)     ~ error: no function `bump` in scope
(5).double()   ~ error: Num has no member `double`
double(5)      ~ 10
```
The same holds for the methods reserved on the built-in types: `"a,b".split(",")` reaches
`Text`'s `split`, and `split("a,b", ",")` is an undefined name.

### Static methods
A method whose body never reads `it` is **static**: it may be called on the TYPE NAME
itself, with no receiver value — the natural spelling for a constructor.
```quilon ignore
Point = {
  x :: Num,
  y :: Num,
  origin = () -> Point => < Point { x = 0, y = 0 } >,
  distance = () -> Num => < it.x >   ~ reads `it` — not static
}

p = Point.origin()      ~ ok: `origin` never reads `it`
Point.distance()        ~ error QN340: `distance` needs a value of Point
p.distance()            ~ ok: called on a value, `it` is bound
```
A method that reads `it` may still be called on an ordinary value, exactly as before —
being static-eligible is a property of a method's body, not a declaration; a call on a
value never needs it.
