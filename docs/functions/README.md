---
title: "Functions"
---

# Functions

```quilon
greet  = => "Hello!"                       ~ no params
double = (x :: Num) => x * 2               ~ one param
add    = (a :: Num, b :: Num) => a + b     ~ multiple params
typed  = (a :: Num, b :: Num) -> Num => a + b
```
Every function parameter must be annotated — there is no default type; an
unannotated parameter is a compile error that names it. The exception is a **lambda**,
which takes its parameter types from whatever receives it whenever that states them (see
[below](#a-lambda-takes-its-parameter-types-from-the-target)) — a built-in collection
method (`.map` / `.filter` / `.reduce` / `.each`) states the element type, and a
function-typed parameter or binding states its own. A **method** parameter is held to the
same rule (see
[named record types](../types/records.md#named-record-types-with-methods)).
Multi-statement bodies use `< >` blocks (the last expression is the value):
```quilon
compute = (x :: Num) => <
  doubled = x * 2
  doubled * doubled
>
```
Functions may recurse; a recursive function needs a `-> Type` annotation:
```quilon
factorial = (n :: Num) -> Num => n == 0 ? 1 : n * factorial(n - 1)
```
(See `examples/factorial.qn`, `examples/fibonacci.qn`.)

## At most ten parameters

A function, method or lambda declares **at most 10 parameters**. The eleventh is a compile
error, reported at the parameter that crosses the line:

```quilon ignore
~ error: a function takes at most 10 parameters — group them into a record type and
~        take that record as one parameter instead
place = (a :: Num, b :: Num, c :: Num, d :: Num, e :: Num, f :: Num,
         g :: Num, h :: Num, i :: Num, j :: Num, k :: Num) -> Num => a
```

Past ten, the arguments are a thing in their own right and want a name. Take a
[record](../types/records.md) instead — it names the group, labels each value at the call
site, and carries **no field limit** of its own:

```quilon
Parcel = { lengthCm :: Num, widthCm :: Num, heightCm :: Num }

volume = (p :: Parcel) -> Num => p.lengthCm * p.widthCm * p.heightCm
```

(See `examples/record_parameter.qn`.)

## Function types & higher-order functions

A **function type** is written with the arrow, reusing `->`. The parameter types go in
parentheses; `$` (Unit) names a function that returns nothing:

```quilon ignore
() -> $              ~ takes nothing, returns unit
(Num) -> Bool        ~ one parameter
(Num, Text) -> Bool  ~ two parameters
```

A function type may be a **parameter type**, which is what makes a function *higher-order*
— it takes another function as an argument and calls it:

```quilon
apply = (f :: (Num) -> Num, x :: Num) -> Num => f(x)
twice = (f :: (Num) -> Num, x :: Num) -> Num => f(f(x))

^ = () -> Num => twice((n) => n * 2, 3)   ~ ((3*2)*2) = 12
```

The value passed in is a closure — a lambda literal (as above) or a named closure passed
by its name. Function types may nest as parameter types (`((Num) -> Bool, Num) -> Bool`).
A function-typed **return** (currying, `(A) -> (B) -> C`) is not supported yet. (See
`examples/higher_order.qn`.)

### A lambda takes its parameter types from the target

Where a lambda lands on a **known function type**, that type says what its parameters are
and the lambda need not repeat them:

```quilon ignore
apply(10, (n) => n + 1)          ~ `apply`'s `(Num) -> Num` parameter types `n`
c.applyTo((n) => n * 2)          ~ so does a method's function-typed parameter
scale :: (Num) -> Num = (n) => n * 4   ~ a binding that declares its function type
```

An annotation is always legal and wins where written (`(n :: Num) => n + 1`). Where the
target type is **not** known, the annotation is required and the error says so rather than
assuming a type:

```quilon ignore
fs = [(n) => n + 1]   ~ error: parameter 'n' of this lambda has no type: annotate it
```

For an **overload set** the target is only known once a member is chosen, so the other
arguments are resolved first. If they pick one member out, that member's parameter types
the lambda; if the set stays open, the lambda must annotate its parameters and the error
names the set that left it open.

## Names resolve top to bottom

A call may only name something **already defined above it** — there is no hoisting. A
definition is in scope for its own body (so a function may recurse) and for everything
that follows it, but not for anything before it:
```quilon ignore
^ = () -> Num => later()   ~ error: Undefined variable 'later'
later = () -> Num => 7
```
This holds for overload-set members too, which report the situation by name:
```quilon ignore
h = () -> Text => g(1)     ~ error: cannot call 'g' before its definition
g = (n :: Num) -> Text => "a"
g = (t :: Text) -> Text => "b"
```
So **mutual recursion between top-level functions is not expressible**: whichever of the
pair comes first would have to call the other before it exists. Self-recursion is
unaffected, including a recursive overload member calling itself. Restructure a mutual
pair into one self-recursive function.
