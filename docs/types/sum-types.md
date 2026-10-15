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
Shape = Circle(Num) / Rect({ Num, Num })   ~ variants with payloads
```
- **A payload is** `Num`, `Text`, `Bool`, `$` (Unit), a declared record, a declared sum —
  the enclosing one included — an anonymous record (named or positional), or an
  array/map of one of those. A `$` payload carries no value — the variant's data is its
  identity (see `Ok($)` below).
- **A variant carries at most one payload.** Several values travel together as one
  record: `Rect({ Num, Num })` constructs as `Rect({ 6, 7 })` and reads back by
  position, `p.0 * p.1`. A variant's parenthesized list naming more than one payload
  type, `Rect(Num, Num)`, is a compile error.
- A **named record** payload lets a sum carry structured data — `Method = Get / Post(Body)`
  where `Body` is a record declared **above** the sum. A match arm binds it at its full
  type, so `Post(b) => b.payload` reads its fields and calls its methods. See
  `examples/nested_composites.qn` for composing sums with records.
- **Two variants may carry different concrete types in their one payload**: `Mixed =
  A(Num) / B(Text)` compiles, each variant reading its own slot through its own typed
  view. `$` costs nothing extra there: `Done($) / Pending(Num)` compiles the same way.
- **A sum may name itself as a payload**, directly (`Tree = Leaf / Node(Tree)`) or
  through an array/map (`Forest = Leaf(Num) / Branch([]Forest)`). A direct self-payload
  is a pointer to a GC cell holding the sum value; through an array or map it sits
  inline, like any other element. See `examples/tree.qn` and `examples/linked_list.qn`.
- **A record may reference itself, or a sum declared above it, directly as a field**
  (`Wagon = { next :: Wagon }`); chained field access (`w.next.next`) reads through it
  like any other record field.
- **Names resolve top to bottom**: a payload or a record field may name the type it sits
  on, or anything declared above it. A name declared below is undeclared at that point —
  the ordinary rule for an undeclared name covers mutual recursion between two types on
  its own, with nothing extra to check.
- **Variant (constructor) names are unique per scope**: each variant name belongs to one
  sum type.

**Construct** a value by naming the variant (with payload arguments if it has any), and
**consume** it with `?`/`|` pattern matching, which binds the payload:
```quilon ignore
area = (s :: Shape) -> Num => <
  s ?
    | Circle(r) => 3 * r * r
    | Rect(p)   => p.0 * p.1        ~ binds the record payload, read by position
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
Shape = Circle(Num) / Rect({ Num, Num }) {
  area = () -> Num => < it ? | Circle(r) => 3 * r * r | Rect(p) => p.0 * p.1 >
  == = (other :: Shape) -> Bool => < it.area() == other.area() >  ~ operator member
  ` = () -> Text => < it ? | Circle(r) => "Circle(`r`)" | Rect(p) => "Rect(`p.0`x`p.1`)" >
}
Rect({ 6, 7 }).area()            ~ 42
```
(See `examples/sum_methods.qn`.)

## `Result` is a normal sum type
`Result` is a predefined sum type, declared as any other:
```quilon ignore
Result = Ok(...) / NotOk(...)    ~ predefined; `Ok` = success, `NotOk` = failure
```
Use it exactly like any other sum type:
```quilon
v = Ok(21)
doubled = v ? | Ok(x) => x * 2    ~ `v` is narrowed to `Ok` by its own construction
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
`examples/result_payload.qn`.) **A member of an [overload set](../functions/overloading.md)
declared `-> Result` carries the payload types its own body returns**, exactly like a
single declaration, independently of every other member sharing its name.

A bare `:: Result` **parameter**'s payload types are the types its own call sites pass —
a method call and a call to one member of an overload set count the same as a plain
function's direct call. A variant no call site passes has no payload type, and a match
arm that reads that binding is
[QN351](../tooling/errors/semantics.md#qn351--unresolved-result-payload); binding it unread, or as
`_`, is allowed. The check covers the functions reachable from `^`, the same set codegen
emits; under `quilon test` the synthesized `^` reaches every test's helpers. See
`examples/result_payload.qn`.

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

## Narrowing `=` bindings

The checker tracks, for an `=` binding or a parameter of a sum type, the set of variants
it can still be — and a match over a narrowed one needs arms only for those:
```quilon
v = Ok(5)
doubled = v ? | Ok(x) => x * 2    ~ exhaustive: `v` is narrowed to `Ok`
```
A parameter narrows the same way (a function's, a method's, or a lambda's), from a proof
made inside the body:
```quilon
Shape = Circle(Num) / Rect({ Num, Num }) / Dot
pick = (s :: Shape) -> Num => <
  assert(s, not(is(Dot)))
  s ? | Circle(r) => r | Rect(p) => p.0
>
```
The fact holds from the point it is established to the end of the enclosing block,
including inside a lambda written after that point (a lambda written before it does not
see it); narrowing established inside a branch or a match arm stays inside that body.

**Three things narrow a binding or parameter:**
- **construction** — `v = Ok(5)`, `s = Circle(5)` (an `=` binding only);
- **`assert`/`expect`** with `isOk()`, `isNotOk()`, the variant matcher `is(Variant)`
  (every sum type, `Result` included — see
  [the matchers](../corelib/test/README.md#the-matchers)), or `not(...)` around any of
  them;
- **inside a match arm** — `s ? | Circle(r) => …` knows `s` is `Circle` for that arm's
  body, when `s` is a bare `=` binding or parameter.

A call's result is never a known variant, whatever the callee's body returns — the proof
stays local to where it was made:
```quilon ignore
loadConfig = () -> Result => < Ok({ port = 8080 }) >
config = loadConfig()
config.port                   ~ `config` carries no narrowing yet — needs a proof below
assert(config, isOk())
```
The binding keeps its sum type throughout: passing, returning, and storing it are
unchanged. Only what a match over it needs to cover narrows.

An arm — a constructor pattern, or a `_`/bound catch-all — that can only cover a variant
already ruled out is a compile error
([QN365](../tooling/errors/semantics.md#qn365--match-arm-a-proven-variant-already-rules-out)),
and so is an `assert`/`expect` that could never hold
([QN364](../tooling/errors/semantics.md#qn364--assertion-a-proven-variant-already-rules-out)):
```quilon ignore
v = Ok(21)
v ? | Ok(x) => x | NotOk(e) => 0    ~ error: `v` is always `Ok` here
assert(v, isNotOk())                 ~ error: `v` is always `Ok`
```
An assertion that always HOLDS stays legal — it is redundant, not an error.

**A `:=` binding is never narrowed** — construction, `assert`/`expect`, and a match arm
all leave it alone. Asserting on one stays legal, an ordinary runtime check; a match over
one stays exhaustive over its full sum, as it does without narrowing at all:
```quilon ignore
m := Ok(5)
assert(m, isOk())      ~ fine — an ordinary runtime assertion
m ? | Ok(x) => x        ~ non-exhaustive — `m`'s variant is not tracked
```

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
