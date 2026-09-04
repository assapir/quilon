---
title: "Sum types — /"
sidebar:
  label: "Sum types"
  order: 3
---

# Sum types — `/`
A sum type (tagged union / enum) is a set of named **variants**, declared with `/`
as the separator. Variants may be **nullary** or carry a payload:
```quilon
Color = Red / Green / Blue                 ~ three nullary variants
Shape = Circle(Num) / Rect(Num, Num)       ~ variants with payloads
```
- **Payloads are built-in scalars or a named record**: `Num`, `Text`, `Bool`, `$`
  (Unit), or a **record** type declared above the sum. A variant may take several payload
  fields (e.g. `Rect(Num, Num)`). A `$` payload carries no value — the variant's data is
  its identity (see `Ok($)` below).
- A **named record** payload lets a sum carry structured data — `Method = Get / Post(Body)`
  where `Body` is a record declared **above** the sum. A match arm binds it at its full
  type, so `Post(b) => b.payload` reads its fields and calls its methods. A payload is a
  scalar or a record; see `examples/nested_composites.qn` for composing sums with records.
- At a given payload position, every variant with a concrete (non-`$`) field there agrees
  on its type, the named-record case included. `$` may coexist with a concrete type at the
  same position: `Done($) / Pending(Num)` is accepted; `A(Num) / B(Text)` and
  `Wrap(Body) / Plain(Num)` are rejected.
- **Variant (constructor) names are unique per scope**: each variant name belongs to one
  sum type.

**Construct** a value by naming the variant (with payload arguments if it has any), and
**consume** it with `?`/`|` pattern matching, which binds the payload:
```quilon ignore
area = (s :: Shape) -> Num => <
  s ?
    | Circle(r)  => 3 * r * r
    | Rect(w, h) => w * h          ~ binds both payload fields
>
```
A match over a sum type **is exhaustive**: it covers every variant, or ends with a `_`
(or a lowercase binding) wildcard. Each pattern names a variant of the scrutinee's type.
(See `examples/sum_types.qn`.)

## Methods — the optional `{ }` block
A sum type may carry a trailing `{ }` block of **methods**. The block is optional: a sum
with no methods is written exactly as above. `it` is the whole sum value, so a method
typically matches on it. A member is a named method, an
[operator](../functions/overloading.md#operator-overloading), or the render `` ` ``. The block holds **methods only**
— a sum has no fields, so a field-like entry there is a compile error, and its methods are
always `=` (see [Mutation](../mutation.md)).
```quilon
Shape = Circle(Num) / Rect(Num, Num) {
  area = () -> Num => < it ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h >
  == = (other :: Shape) -> Bool => < it.area() == other.area() >  ~ operator member
  ` = () -> Text => < it ? | Circle(r) => "Circle(`r`)" | Rect(w, h) => "Rect(`w`x`h`)" >
}
Rect(6, 7).area()                ~ 42
```
(See `examples/sum_methods.qn`.)

## `Result` is a normal sum type
`Result` is a predefined sum type, declared as any other:
```quilon ignore
Result = Ok(...) / NotOk(...)    ~ predefined; `Ok` = success, `NotOk` = failure
```
Use it exactly like any other sum type:
```quilon
classify = (v :: Result) => <
  v ?
    | Ok(x)    => x * 2
    | NotOk(e) => 0
>
```
Payloads work end-to-end for `Num`, `Bool`, and `Text` (e.g. `Ok("done")` /
`NotOk("error")`). A **pattern-bound payload carries its concrete type**, so it is
*usable* at the match site: `Ok("x") ? Ok(s) => s.size` binds `s : Text`, and passing
`s` to an [overload set](../functions/overloading.md) dispatches to the `Text` member.
`Ok` and `NotOk` each carry their own concrete type at a given position, and the two
may differ: `Ok(Text) / NotOk(Num)` is a normal `Result` (the `getOpt` shape below is
one example). Within one variant, every value carries the same concrete type at a
given position.
An **array literal of `Result`s** unifies each variant's payload type across every
element: `[Ok("a"), NotOk("b")]` carries a `Text` payload for BOTH `Ok` and `NotOk`,
however the elements are ordered, so a later `.map`/match on any element binds its
real payload type.
This holds across a function boundary too. A function returning `Ok("x")` —
return type inferred or annotated `-> Result` — hands the caller a usable `Text` payload.
And a `-> Result` whose branches are `Ok(Text)` / `NotOk(Text)` — the `getEnv`/`getOpt`
shape — carries **both** arms' payloads. (See `examples/result.qn` and
`examples/result_payload.qn`.)

Every `Result` shares **one uniform layout** regardless of its payload, so a `Result`
carrying *any* payload — `Num`, `Text`, `[]Text`, a composite — passes through a generic
`(r :: Result)` parameter or return. This is what lets the `isOk()` / `isNotOk()`
[matchers](../corelib/test/README.md#the-matchers) read a `Result` of any shape, including the
composite-payload results of `getEnv` / `getOpt` (see `examples/cli.qn`). Extracting a
payload needs its concrete type in scope at the match site, and *matching by variant*
(`Ok` / `NotOk`) works on any `Result` anywhere.

A constructor pattern's argument must be **irrefutable** — a binding (`Ok(x)`) or the
wildcard (`Ok(_)`). A literal or nested constructor there (`Ok(1)`, `Ok(Ok(x))`) is a
compile error: such a pattern would silently match *any* payload of the variant. Bind the
payload and compare it in the arm body instead (`Ok(n) => n == 1 ? … : …`).

## `/` — sum-type separator vs. division
`/` is the division operator **and** the sum-type variant separator. Quilon tells them
apart by its **Capitalized-type / lowercase-value** convention. `/` is a variant separator
**only** in a type-declaration context — that is, when the binding name and every operand
are Capitalized type/constructor names:
```quilon ignore
Color = Red / Green / Blue       ~ sum type: name + operands are Capitalized
half  = a / b                    ~ division: lowercase operands are values
```
A single bare Capitalized name with no `/` (e.g. `x = Red`) is an ordinary value binding
(here, of an existing nullary variant); a sum-type declaration has two or more variants.
