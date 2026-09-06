---
title: "Collections"
---

# Collections

Quilon has three built-in parametric collections: arrays, maps, and sets.

## Arrays

An array `[]T` is the **base built-in parametric collection** — the one Maps and Sets
define themselves against — written as a bracket literal (`[1, 2, 3]`). A `:=`-bound array
supports an in-place element write, `arr[i] := value`; every other operation (`+`, the
array methods) builds a new array. `.size` counts its elements. Indexing (`nums[0]`) is
checked and fails loud; `at(n)` is the `Ok`/`NotOk`
form. Its built-in methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) chain freely, and
`+` builds a new array. Full reference: [`docs/collections/arrays.md`](arrays.md) (and
`examples/arrays.qn`, `examples/array_methods.qn`, `examples/array_concat.qn`).

## Maps

A `Map` is a **built-in parametric collection**, like [`[]T`](#arrays), written with a
**pipe fence** `[|K => V|]` (`=>` reads "maps to"). It is keyed by
`Num`/`Text`/`Bool` or a **user type** that defines both a `%` hash hook and an `==` member,
and read through `.get`, which returns a `Result`. `set`/`remove` are setters: they mutate
a `:=`-bound map in place and return `it`.
Full reference: [`docs/collections/map.md`](map.md) (and `examples/maps.qn`).

## Sets

A `Set` is a **built-in parametric collection**, like [`[]T`](#arrays), written with the
same **pipe fence** `[|T|]`, which distinguishes a set literal from an array.
It holds unique `Num`/`Text`/`Bool` elements (or a **user type** defining both
a `%` hash hook and an `==` member), and supports set algebra
(`+` union, `-` difference, `+-`/`-+` intersection), each building a new set. `add`/`remove`
are setters: they mutate a `:=`-bound set in place and return `it`. Full reference:
[`docs/collections/set.md`](set.md) (and `examples/sets.qn`).
