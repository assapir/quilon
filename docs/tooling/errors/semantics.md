---
title: "Semantic errors"
sidebar:
  order: 3
---

# Semantic errors

QN3xx: the type checker's errors.

## Checker

### QN300 — undefined name

A name is used with no definition above it. Names resolve top to bottom.

```quilon ignore
^ = () -> Num => < total >
```

Define the name before its use: `total = 1` on a line above.

### QN301 — type mismatch

An expression has one type where another is required.

```quilon ignore
x :: Num = "seven"
```

Give the binding a value of the annotated type, or change the annotation to match.

### QN302 — call on a data value

A value of a data type is called with `( )`.

```quilon ignore
^ = () -> Num => < n = 1  n(2) >
```

Call a function, or index a collection with `[ ]`.

### QN303 — wrong number of arguments

A call passes more or fewer arguments than the function declares.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
^ = () -> Num => < double(1, 2) >
```

Pass exactly the declared parameters: `double(1)`.

### QN304 — assignment to an immutable binding

A binding made with `=` is reassigned.

```quilon ignore
^ = () -> Num => < n = 1  n := 2 >
```

Bind it with `:=` to allow writes: `n := 1`.

### QN305 — write through an immutable binding

A record field or array element is written through a binding made with `=`.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p = Point { x = 1 }  p.x := 2 >
```

Bind the record with `:=`: `p := Point { x = 1 }`. The same applies to an array element
write (`arr[i] := v`) on an `=`-bound array.

### QN306 — `:=` binding aliasing an immutable value

A `:=` binding takes the value of an `=` binding, a parameter, or the receiver `it` of an
`=` method. A value bound with `=` stays immutable through every alias. Only reference
types are checked — a record, or an array/`Set`/`Map` holding one; an array of `Num`,
`Bool`, or `Text` copies freely and is exempt.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p = Point { x = 1 }  q := p >
```

Bind with `=`, or build a fresh value: `q := Point { x = p.x }`.

### QN307 — `=` binding aliasing a mutable value

An `=` binding takes the value of a `:=` binding; writes through the mutable binding would
change the `=`-bound value. Only reference types are checked, the same as QN306 above.

```quilon ignore
Point = { x :: Num }
^ = () -> Num => < p := Point { x = 1 }  q = p >
```

Bind with `:=`, or build a fresh value.

### QN308 — mutating method on an immutable receiver

A `:=` method is called on a receiver bound with `=`. The same applies to a built-in
`Map`/`Set` mutator (`set`/`remove` on a `Map`, `add`/`remove` on a `Set`).

```quilon ignore
Counter = { n :: Num, bump := () -> $ => < it.n := it.n + 1 > }
^ = () -> Num => < c = Counter { n = 0 }  c.bump() >
```

Bind the receiver with `:=`: `c := Counter { n = 0 }`.

### QN309 — mutating method declared with `=`

A method declared with `=` writes to `it`.

```quilon ignore
Counter = { n :: Num, bump = () -> $ => < it.n := it.n + 1 > }
```

Declare it with `:=`: `bump := () -> $ => < … >`. A lambda parameter named `it` inside the
body shadows the receiver; rename it where the write targets the lambda's own value.

### QN310 — duplicate definition

A name is defined twice in one scope, and the two definitions form no overload set.
A record's fields and methods, a constructor or record literal's fields, a map literal's
literal keys, and the members of one overload set are each one scope of their own.

```quilon ignore
x = 1
x = 2
```

Give each definition its own name.

### QN311 — no matching overload

A call, or an operator, has argument types that match no member of its overload set.
Dispatch is by exact type.

```quilon ignore
^ = () -> Num => < 1 + "x" >
```

Pass the types a member takes; the `help:` line lists the members. To join a number and
text, interpolate: `` "`n`x" ``.

### QN312 — ambiguous overload

More than one member of an overload set matches a call's argument types. Two members with
the very same parameter types are rejected up front, as a
[duplicate definition](#qn310--duplicate-definition) — this is the narrower case a call site
finds ambiguous: a member taking a trailing `site :: Site` the compiler fills in
automatically, alongside one without it, both accept a call that omits it.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (n :: Num, site :: Site) -> Num => < n >
^ = () -> Num => < f(1) >
```

Give the members distinct parameter types, or call with enough arguments to rule one out.

### QN313 — overload member with an unannotated parameter

A member of an overload set leaves a parameter without a type. Exact dispatch reads every
member's full signature.

```quilon ignore
f = (n :: Num) -> Num => < n >
f = (t) -> Num => < t.size >
```

Annotate every parameter: `f = (t :: Text) -> Num => < t.size >`.

### QN314 — parameter without a type

A function parameter has no type annotation and no context to take one from.

```quilon ignore
double = (x) => < x * 2 >
```

Annotate the parameter: `double = (x :: Num) => < x * 2 >`.

### QN315 — parameter count differs from the function type

A definition, or a lambda, declares a different number of parameters than the function type
it must match.

```quilon ignore
f :: (Num, Num) -> Num = (a :: Num) => < a >
```

Declare exactly the parameters the type states.

### QN316 — lambda parameter with an open type

A lambda leaves a parameter unannotated in a position that states no function type — an
array element, a plain expression, a sum payload — or an overload set the other arguments
leave open.

```quilon ignore
^ = () -> Num => < fns = [x => x + 1] >
```

Annotate the parameter: `[(x :: Num) => x + 1]`.

### QN317 — recursive function without a return type

A function calls itself and declares no `-> T`. A call to the function needs its return
type before the body is checked.

```quilon ignore
fact = (n :: Num) => < n <= 1 ? 1 : n * fact(n - 1) >
```

Annotate the return type: `fact = (n :: Num) -> Num => < … >`.

### QN318 — write to a `Site` field

A field of a `Site` is assigned. A location is a value.

```quilon ignore
f = (site :: Site) -> $ => < site.line := 1 >
```

Read the fields; build a fresh record where a different location is wanted.

### QN319 — misplaced `Site` parameter

A `Site` parameter sits anywhere but last in a top-level function, or on a lambda or
method. The compiler fills a call site only as the last parameter of a top-level function.

```quilon ignore
check = (site :: Site, n :: Num) -> $ => < $ >
```

Move the `Site` parameter last: `check = (n :: Num, site :: Site) -> $ => < $ >`.

### QN320 — call before the definition

A call reaches an overload set — two or more same-named definitions — every member of
which is defined below the call. A name with only ONE definition below the call is instead
[undefined](#qn300--undefined-name): names resolve top to bottom, and a set forms at its
second member.

```quilon ignore
h = () -> Text => < g(1) >
g = (n :: Num) -> Text => < "a" >
g = (t :: Text) -> Text => < "b" >
```

Move the definitions above the call.

### QN321 — overload member without a return type

A member of an overload set omits its `-> T`. Exact dispatch reads the full signature.

```quilon ignore
f = (n :: Num) => < n >
f = (t :: Text) -> Num => < t.size >
```

Annotate the return type: `f = (n :: Num) -> Num => < n >`.

### QN322 — comparison operator returning a type other than `Bool`

A member for `==`, `!=`, `<`, `<=`, `>`, or `>=` declares a return type other than `Bool`.

```quilon ignore
Point = { x :: Num, == = (other :: Point) -> Num => < 0 > }
```

Return `Bool`: `== = (other :: Point) -> Bool => < it.x == other.x >`.

### QN323 — nested pattern inside a constructor pattern

A constructor pattern's argument is a literal or another constructor. The match dispatches
on the constructor tag alone.

```quilon ignore
^ = () -> Num => < r = Ok(1)  r ? | Ok(1) => 1 | _ => 0 >
```

Bind the payload and compare it in the arm: `| Ok(n) => n == 1 ? 1 : 0`.

### QN324 — non-exhaustive match

A `?` match leaves values uncovered — a sum type with variants no arm lists, or any other
type with no `_` arm.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Red => 1 >
```

Add the missing arms, or a `_` arm.

### QN325 — unknown variant

A constructor pattern names a variant the sum type lacks.

```quilon ignore
Color = Red / Green
f = (c :: Color) -> Num => < c ? | Blue => 1 | _ => 0 >
```

Name one of the type's variants; the message lists them.

### QN326 — constructor pattern on a non-sum value

A constructor pattern matches a value of a type with no variants, or one whose type is
open at the match.

```quilon ignore
^ = () -> Num => < 5 ? | Ok(x) => x | _ => 0 >
```

Match the value itself — a literal, a binding, or `_` — or annotate the value's type.

### QN327 — unsupported `^` signature

The entry point declares parameters other than the supported forms.

```quilon ignore
^ = (n :: Num) -> Num => < n >
```

Declare `^` as `()`, `(args :: []Text)`, or `(args :: []Text, env :: [|Text => Text|])`.

### QN328 — invalid argument to a built-in

A built-in method receives an argument outside its contract that is visible statically —
`Text.repeat` with a negative literal count, an index into a `Map`, an empty `< >` block
where a value is required.

```quilon ignore
^ = () -> Num => < "ab".repeat(-1).size >
```

The message states the contract; pass an argument that meets it.

### QN329 — mutable global exported

`>>` marks a top-level `:=` binding. A `:=` global is one mutable cell for the whole
program; mutation does not cross a module boundary.

```quilon ignore
>> secretSauce := 0
```

Export a function that reads or writes it instead.

### QN330 — operator defined at the top level

An operator symbol names a top-level definition. An operator is a member of the record or
sum type it operates on.

```quilon ignore
+ = (a :: Point, b :: Point) -> Point => < Point { x = a.x + b.x } >
```

Define it inside the type's `{ }`, where `it` is the left operand:
`+ = (other :: Point) -> Point => < … >`.

### QN331 — operator member with the wrong parameter count

An operator member declares more or fewer than one explicit parameter. `it` is the left
operand; the one parameter is the right.

```quilon ignore
Point = { x :: Num, + = (a :: Point, b :: Point) -> Point => < a > }
```

Declare one parameter: `+ = (other :: Point) -> Point => < … >`.

### QN332 — assertion without a matcher

`assert` or `expect` is called with anything but a value and one of the matchers.

```quilon ignore
^ = () -> $ => < assert(1 == 1) >
```

Pass the value and a matcher: `assert(1, equals(1))`. The matchers are `equals`,
`contains`, `not`, `isOk`, `isNotOk`.

### QN333 — `expect` outside a test case

`expect` is called outside an `it` case of a `describe` block.

```quilon ignore
^ = () -> $ => < expect(1, equals(1)) >
```

Use `assert`, which reports and exits, outside a test case.

### QN334 — matcher with the wrong argument count

A matcher is called with more or fewer arguments than it takes.

```quilon ignore
^ = () -> $ => < assert(1, equals(1, 2)) >
```

Pass the matcher's arguments: `equals(1)`.

### QN335 — matcher on a type outside its reach

A matcher meets a type outside its reach: `equals` on a type without a `==` member,
`contains` on anything but `Text` or an array, `isOk`/`isNotOk` on anything but a `Result`.

```quilon ignore
^ = () -> $ => < assert(1, contains(1)) >
```

Apply a matcher that reads the type: `assert([1], contains(1))`.

### QN336 — unknown member

`value.name` names a member the value's type lacks. A function of the same name in scope
answers the plain call form only.

```quilon ignore
<< core.io
^ = () -> Num => < n = 1  n.print()  0 >
```

Call the function on the value: `io.print(n)`.

### QN337 — method called as a function

`name(value)` names a member of the value's type, and the top level has no function of that
name.

```quilon ignore
Counter = { n :: Num, bumped = () -> Num => < it.n + 1 > }
^ = () -> Num => < c = Counter { n = 1 }  bumped(c) >
```

Call the member on the value: `c.bumped()`.

### QN338 — value with no rendering

`io.print`, `io.eprint`, or `io.write` receives a value with no `` ` `` render member — a
function.

```quilon ignore
<< core.io
f = () -> Num => < 1 >
^ = () -> Num => < io.print(f)  0 >
```

Print a renderable value, or give the type a `` ` `` member.

### QN339 — no `^` entry point

`run`, `build`, or `compile` is given a file with no `^` function.

```quilon ignore
double = (x :: Num) -> Num => < x * 2 >
```

Define the entry point: `^ = () -> Num => < 0 >`.

### QN340 — static call on a method that reads its receiver

A method called on the bare TYPE NAME (`Point.origin()`, not a value) reads the receiver
`it` in its body — there is no value to bind it to. Only a STATIC method — one that never
reads `it`, the natural spelling for a constructor (`Request.get(url)`) — may be called
this way.

```quilon ignore
Point = { x :: Num, distance = () -> Num => < it.x > }
^ = () -> Num => < Point.distance() >
```

Call it on a value instead: `p.distance()`.

### QN341 — store into a mutable container aliasing an immutable value

A field write, or a setter-call argument the setter stores into `it`, stores a value that
aliases an `=` binding or a parameter into a `:=`-reachable container. This is QN306's own
rule, applied at the store as well as the binding: a value bound with `=` stays immutable
through every alias, so a store cannot make it reachable through a `:=` binding either.

```quilon ignore
Counter = { value :: Num }
Box = { item :: Counter }
^ = () -> Num => <
  c = Counter { value = 30 }
  b := Box { item = Counter { value = 1 } }
  b.item := c
  c.value
>
```

Store a fresh value: `b.item := Counter { value = c.value }`.

### QN342 — missing constructor field

A record constructor leaves out one of its type's declared fields.

```quilon ignore
P = { x :: Num, y :: Num }
^ = () -> Num => < p = P { x = 1 }  0 >
```

Add the missing field: `P { x = 1, y = 2 }`.

### QN343 — unknown constructor field

A record constructor supplies a field its type does not declare.

```quilon ignore
P = { x :: Num, y :: Num }
^ = () -> Num => < p = P { x = 1, y = 2, z = 3 }  0 >
```

Remove the field, or fix its name against the type's declared fields.

### QN344 — reserved name

A binding — a type, a `=`/`:=` binding, a function, a parameter, a lambda parameter, or a
pattern binding — under a name the language reserves. The reserved names are the built-in
types `Num`, `Bool`, `Text`, `Result`, `Site`, `Map`, `Set`; the constructors `Ok` and
`NotOk`; the receiver `it`; the assertions `assert` and `expect`; and the matchers
`equals`, `contains`, `not`, `isOk`, `isNotOk`. The message names what the word is reserved
for. A member — a record field or method — is not a binding and may carry any of these
names.

```quilon ignore
not = (b :: Bool) -> Bool => < !b >
```

Pick another name.

### QN345 — record field with a function type

A record field declared with a function type. A field holds data; a function member of a
record is written as a method instead.

```quilon ignore
Box = { scale :: (Num) -> Num }
^ = () -> Num => < 0 >
```

Write `scale` as a method: `scale = (n :: Num) -> Num => < n * 2 >`.

### QN346 — unsupported sum-type payload

A sum-type variant payload of a type outside the accepted set: `Num`, `Text`, `Bool`, `$`,
a declared record, a declared sum (the enclosing one included), or an array/map of one of
those. The message names the variant, the payload's position, and the type it resolved to.

```quilon ignore
Mystery = Wrap(Nope) / Empty
^ = () -> Num => < 0 >
```

`Nope` names nothing declared above `Mystery`. A payload is Num, Text, Bool, $, a declared
record, a declared sum, or an array/map of one of those.

### QN347 — atomic binding declared without `:=`

`@name = value` declares an atomic binding — `@name := value` — with `=` instead. Atomicity
is a property of a mutable binding's reassignment; an immutable binding never reassigns.

```quilon ignore
@luckyNumber = 7
^ = () -> Num => < luckyNumber >
```

Declare it with `:=`: `@luckyNumber := 7`.

### QN348 — atomic binding used with `@` after its declaration

`@` appears on an atomic binding outside its declaration — reading it, or reassigning it —
where the bare name is already in scope. `@` marks the declaration only.

```quilon ignore
^ = () -> Num => <
  @hits := 0
  @hits := hits + 1
>
```

Read or reassign it bare: `hits := hits + 1`.

### QN349 — atomic binding reassignment waits on a deferred value

An atomic binding's reassignment executes as a whole: its right side forces a deferred
value — a value-returning `@` primitive's result read strictly (arithmetic, comparison,
a field, a native call, …) — which would park the fiber mid-statement.

```quilon ignore
<< core.io

@hits := 0
bump = () -> $ => < hits := hits + io.@readStdin().length >
^ = () -> $ => < bump() >
```

Force the value into a plain `=` binding first, then reassign:

```quilon
<< core.io

@hits := 0
bump = () -> $ => <
  extra = io.@readStdin().length
  hits := hits + extra
>
^ = () -> $ => < bump() >
```

### QN350 — `:=` value shared across fibers

A non-atomic `:=` value — a top-level binding, or a local of the block the call sits in —
is read or written by the `handler` argument of `net.@tcpServe`, or by anything `handler`
calls transitively. `handler` runs on its own fiber per connection, so every non-atomic
`:=` value it reaches is shared with whichever fiber declared it.

```quilon ignore
<< core.net

hits := 0
^ = () -> Num => <
  net.@tcpServe("127.0.0.1:9048", connection => < hits := hits + 1 >)
  0
>
```

Bind it atomically:

```quilon
<< core.net

@hits := 0
^ = () -> Num => <
  net.@tcpServe("127.0.0.1:9048", connection => < hits := hits + 1 >)
  0
>
```

### QN351 — unresolved `Result` payload

A `Result` parameter's payload types are the types its own call sites pass — a method
call (pinned from its receiver's own call) and a call to one member of an overload set
(pinned from the argument types that resolved it) count exactly like a plain function's
direct call. A variant no call site passes has no payload type, and a match arm that
reads that binding is this error.

```quilon ignore
describe = (result :: Result) -> Text => <
  result ? | Ok(text) => text | NotOk(_) => "none"
>
~ Every caller passes NotOk, so Ok has no payload type for `text` to read.
^ = () -> $ => < assert(describe(NotOk("lost")), equals("none")) >
```

A caller that passes `Ok` gives `text` its type — a method call works the same way:

```quilon
Parcel = {
  label :: Text,
  describe = (result :: Result) -> Text => <
    result ? | Ok(text) => text | NotOk(_) => "none"
  >
}
^ = () -> $ => < assert(Parcel { label = "x" }.describe(Ok("home")), equals("home")) >
```

The check covers the functions reachable from `^`, the same set codegen emits (a helper
only called from inside a `test.describe`/`test.it` block, which `quilon run`/`check`/
`build` erase, is outside that set under those commands); under `quilon test` the
synthesized `^` reaches every test's helpers.

### QN352 — top-level function never called

A non-exported top-level function is a module's private detail — nothing outside its own
file can name it. Nothing inside it calls this one either: not `^`, and — in a module with
no `^` of its own — not any of the module's `>>`-exported functions. A function nothing can
reach is dead: it has no way to run.

```quilon ignore
>> grind = (n :: Num) -> Num => < n * 2 >
sift = (n :: Num) -> Num => < n + 1 >
```

`sift` is called, exported, or gone:

```quilon
sift = (n :: Num) -> Num => < n + 1 >
>> grind = (n :: Num) -> Num => < n * 2 + sift(n) >
```

### QN353 — top-level function reachable only from test blocks

`run`, `build` and `check` erase a file's `test.describe` blocks before compiling it — only
`quilon test`'s synthesized `^` runs them. A function only those blocks mention is dead in
the program; testing a function nothing uses has no meaning, so this is its own error.

```quilon ignore
<< core.test
describe = (result :: Result) -> Text => <
  result ? | Ok(text) => text | NotOk(_) => "none"
>
test.describe("describe", () => <
  test.it("ok", () => < expect(describe(Ok("home")), equals("home")) >)
>)
^ = () -> Num => < 0 >
```

`quilon test` runs the block and passes; `quilon run`/`build`/`check` raise QN353 until
`describe` is called from `^`, exported, or its test moves with it:

```quilon
<< core.test
describe = (result :: Result) -> Text => <
  result ? | Ok(text) => text | NotOk(_) => "none"
>
test.describe("describe", () => <
  test.it("ok", () => < expect(describe(Ok("home")), equals("home")) >)
>)
^ = () -> Num => < describe(Ok("home")).size >
```

### QN354 — signal trap outside the root file

A [signal trap](../../concurrency/signals.md) (`!>`) declared in a file other
than the one that defines `^` — here, an imported module.

```quilon ignore
<< "traps.qn"

^ = () -> Num => < 0 >
```

```quilon title="traps.qn"
<< core.process

!> | Interrupt(s) => 1
```

Move the trap to the file that defines `^`.

### QN355 — second signal trap

A program declares at most one signal trap.

```quilon ignore
<< core.process

!> | Interrupt(s) => 1

!> | Terminate(s) => 2

^ = () -> Num => < 0 >
```

Merge the arms into one trap.

### QN356 — signal trap without `core.process`

A trap's arms match [`process.Signal`](../../corelib/process.md), which needs `<<
core.process` — missing here.

```quilon ignore
!> | Interrupt(s) => 1

^ = () -> Num => < 0 >
```

Add `<< core.process`.

### QN357 — signal trap arm without a signal pattern

A trap arm's pattern must name a `process.Signal` variant — a bare binding, `_`, or a
literal binds nothing the runtime could dispatch a concrete signal to.

```quilon ignore
<< core.process

!> | anySignal => 1

^ = () -> Num => < 0 >
```

Match one of `Signal`'s variants, e.g. `| Interrupt(sender) => …`.

### QN358 — duplicate signal trap arm

Two arms of the same trap name the same `process.Signal` variant.

```quilon ignore
<< core.process

!> | Interrupt(s) => 1
   | Interrupt(s) => 2

^ = () -> Num => < 0 >
```

A trap has at most one arm per signal — remove or merge the duplicate.
