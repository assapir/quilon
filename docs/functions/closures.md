---
title: "Closures and tail recursion"
---

# Closures and tail recursion

## Tail self-recursion runs in constant stack (guaranteed)

A self-call is in **tail position** when it is the function's whole result. When a
function returns such a call, the compiler **guarantees** constant stack: a tail-recursive
function runs in **constant stack** at any depth:
```quilon
count = (n :: Num, acc :: Num) -> Num => <
  n == 0 ? acc : count(n - 1, acc + n)   ~ the self-call IS the `:` branch → tail position
>
```
Tail position flows through the constructs that yield a value directly: `?`/`|` match
arms, ternary branches, and the tail of a `< >` block. A self-call outside tail position
is ordinary recursion (e.g. `n * fact(n - 1)`, whose result is multiplied first), and so
is a tail call to a *different* function. The guarantee applies as written, with no
marker.
(See `examples/tail_recursion.qn`, which recurses 1,000,000 deep.)

## Closures — capture by `=` (value) vs `:=` (reference)

A function written **inside** another function's body is a **closure**: it can read the
enclosing locals it refers to. How each name is captured is decided by **the operator that
bound it** — no capture list, no marker, mirroring the mutability rule for
[variables](../variables.md) and [records](../mutation.md):

- **`=`** captures **by value** — a frozen snapshot taken when the closure is created;
- **`:=`** captures **by reference** — one shared mutable cell. Writes through it, from
  inside the closure or outside, are visible to everyone sharing it, and the cell survives
  the frame that created it.

```quilon
^ = () -> Num => <
  total := 0                 ~ `:=` -> captured BY REFERENCE
  bump = (n :: Num) => <
    total := total + n       ~ writes the SHARED cell; the effect persists across calls
    total
  >
  bump(10)                   ~ total -> 10
  bump(20)                   ~ total -> 30  (same cell)

  base = 7                   ~ `=`  -> captured BY VALUE (a frozen copy)
  addBase = (x :: Num) => < x + base >

  total + addBase(5)         ~ 30 + 12 = 42
>
```

A non-capturing nested function may **recurse** (`fact = (n :: Num) => < … fact(n-1) … >`).
Nested closures may capture from any enclosing frame — the shared `:=` cell is threaded
through every level. A closure value may itself be captured by another closure and called.
A closure may also be **passed to a function** whose parameter has the matching
[function type](README.md#function-types--higher-order-functions) and called there.

Closures are **monomorphic**: parameters and captured values are concrete-typed. Capturing
a polymorphic value and generic closures are deferred — see
[Known limitations](../status/limitations.md).

## Returning a closure

A function's result may itself be a function: the return is written as a
[function type](README.md#function-types--higher-order-functions), and the body hands back
a closure. Its captures live on the **GC heap**, so they outlive the frame that made them
— by-value (`=`) snapshots and shared `:=` cells alike:

```quilon
adder = (n :: Num) -> (Num) -> Num => < (x) => x + n >

mkCounter = () -> () -> Num => <
  count := 0                 ~ the `:=` cell survives mkCounter's return
  () -> Num => <
    count := count + 1
    count
  >
>

^ = () -> Num => <
  add5 = adder(5)            ~ call it through a binding…
  seven = add5(2)
  answer = adder(40)(2)      ~ …or immediately: a call on a function-valued expression
  tick = mkCounter()
  tick()                     ~ 1
  seven + answer + tick()    ~ 7 + 42 + 2 = 51
>
```

The declared return type is a **contextual-typing position**: the lambda handed back takes
its parameter types from it (`(x) => x + n` above writes no annotation — the return says
`x` is a `Num`), exactly as a lambda argument takes them from the receiving signature. A
returned closure is an ordinary function value: bind it, call it, pass it on to another
function, or return it from another closure.

What is handed back is a closure value — a lambda literal, a named closure binding,
or a function-typed parameter. A **top-level function** is handed back through a lambda
that calls it (see [Known limitations](../status/limitations.md)). (See
`examples/closures.qn`.)
