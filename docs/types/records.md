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

A field declared twice is a duplicate definition, whatever type each declaration gives it.
A field and a method may share a name: a bare access (`it.a`) always reaches the field, and
a dot-call (`it.a(...)`) always reaches the method, so the two never compete — a field
holds data, and a function member of a record is written as a method. Two or more methods
sharing a name form an [overload set](../functions/overloading.md#method-overloading), each
member fully annotated and dispatched by exact argument type; two members with the same
signature are a duplicate definition. A constructor or record literal names each field once
— a `<-source` spread fills fields, and a later literal may override one of them, but the
same field written twice as a literal is a duplicate.

A field's type is never a function — a function member of a record is a method, not a
field (`Box = { scale :: (Num) -> Num }` is a compile error naming the field and pointing
at the method form). A method's parameters, a binding's declared type, and a function's
return type may all be function-typed.
```quilon
Request = {
  options :: Num,
  options = (n :: Num) -> Request => < Request { options = n } >
}

^ = () -> Num => <
  r = Request.options(3)  ~ dot-call on the type name reaches the static method
  r.options                ~ bare access reaches the field
>
```
(See `examples/methods.qn`, which also exercises a value-receiver method sharing a
field's name.)

## Anonymous record types in annotations
A record type may be written directly in an annotation — a parameter, a return type, a
binding, a named record's own field type, or an array/map element type — without first
declaring it a name. Written with field names, it is the same shape a record LITERAL
builds:
```quilon
greet = (guest :: { name :: Text, age :: Num }) -> Text => < "Hi, " + guest.name >

^ = () -> Num => <
  greet({ name = "Wu", age = 41 })
  0
>
```
Two anonymous record types with the same fields (same names, same types, same order) are
the same type — there is nothing further to declare for them to match. A NAMED record
type (`User = { name :: Text, age :: Num }`) never converts to or from an anonymous one
implicitly, in either direction — a `User` and `{ name :: Text, age :: Num }` are
distinct types even though their fields line up:
```quilon
User = { name :: Text, age :: Num }
greet = (guest :: { name :: Text, age :: Num }) -> Text => < "Hi, " + guest.name >

^ = () -> Num => <
  u = User { name = "Wu", age = 41 }
  greet({ name = u.name, age = u.age })  ~ rebuild it field by field
  0
>
```
```quilon ignore
greet(u)   ~ error: expected { name :: Text, age :: Num }, got User
```
Going the other way, an anonymous record built to a named type's exact shape converts
explicitly with the spread constructor: `User {<-guest}` (see
[Spread](../expressions/README.md) and the functional-update form above).

## Positional records
A record's fields may be positional instead of named — written as a bare list of types
(`{ Num, Num }`) or values (`{ 6, 7 }`), read back by position: `.0`, `.1`, and so on. A
record is all named or all positional; `{ Num, label :: Text }` and `{ 1, label = "x" }`
each mix the two and are a compile error.
```quilon
loot = { 12, 3 }        ~ a positional record — no names, read back by position
coins = loot.0
gems = loot.1
```
A one-field positional record (`{ Num }`) is allowed, read with `.0` — useful when a
type starts with one value and may grow a second later without renaming anything. `.0`
on a NAMED record (anonymous or declared) is a compile error, and a position past a
record's last field is too:
```quilon ignore
loot.5          ~ error: loot has 2 field(s), positions 0..1
```
A position is only ever digits directly after a `.` — `pair.0.1` is two field accesses
(`.0` then `.1`) on a nested positional record, never the number `0.1`:
```quilon
grid = { { 1, 2 }, { 3, 4 } }
^ = () -> Num => < grid.0.1 + grid.1.0 >   ~ 2 + 3
```

### Static methods
A method whose body never reads `it` is **static**: it may be called on the type name
itself — the natural spelling for a constructor.
```quilon
Point = {
  x :: Num,
  y :: Num,
  origin = () -> Point => < Point { x = 0, y = 0 } >,
  distance = () -> Num => < it.x >   ~ reads `it`: called on a value
}

^ = () -> Num => <
  p = Point.origin()  ~ ok: `origin` never reads `it`
  p.distance()         ~ ok: called on a value, `it` is bound
>
```
A static method is also callable on a value, the same way as any other method.
```quilon ignore
Point.distance()        ~ error QN340: `distance` needs a value of Point
```
A static method may be overloaded: two or more same-named static members dispatch on the
bare type name by exact argument type, exactly as a value-receiver overload dispatches on
a value.
```quilon
Segment = {
  start :: Num,
  end   :: Num,
  make = (end :: Num) -> Segment => < Segment { start = 0, end = end } >,
  make = (start :: Num, end :: Num) -> Segment => < Segment { start = start, end = end } >
}

^ = () -> Num => <
  a = Segment.make(5)      ~ start = 0, end = 5
  b = Segment.make(2, 5)   ~ start = 2, end = 5
  a.end + b.start
>
```
