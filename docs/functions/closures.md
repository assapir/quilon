---
title: "Closures and tail recursion"
---

# Closures and tail recursion

## Tail self-recursion runs in constant stack (guaranteed)

A self-call is in **tail position** when it is the function's whole result, with nothing
left to do to it. When a function returns such a call, the compiler **guarantees** the
call does not grow the stack. So a tail-recursive function runs in **constant stack** and
will not overflow, however deep the recursion:
```quilon
count = (n :: Num, acc :: Num) -> Num =>
  n == 0 ? acc : count(n - 1, acc + n)   ~ the self-call IS the `:` branch → tail position
```
Tail position flows through the constructs that yield a value directly: `?`/`|` match
arms, `if`/ternary branches, the tail of a `< >` block, and a `|>` pipeline. A self-call
**not** in tail position stays ordinary recursion (e.g. `n * fact(n - 1)`, whose result
is multiplied first). So does a tail call to a *different* function — general/mutual tail
calls are a later follow-up. There is no surface syntax for it: nothing is written to ask
for the guarantee.
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
  addBase = (x :: Num) => x + base

  total + addBase(5)         ~ 30 + 12 = 42
>
```

A non-capturing nested function may **recurse** (`fact = (n :: Num) => … fact(n-1) …`).
Nested closures may capture from any enclosing frame — the shared `:=` cell is threaded
through every level. A closure value may itself be captured by another closure and called.
A closure may also be **passed to a function** whose parameter has the matching
[function type](README.md#function-types--higher-order-functions) and called there.

Closures are **monomorphic**: parameters and captured values are concrete-typed. Capturing a
polymorphic value, generic closures, and **returning** a closure across frames are deferred
— see [Known limitations](../status/limitations.md). (See `examples/closures.qn`.)
