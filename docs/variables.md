---
title: "Variables"
---

# Variables

`=` declares an immutable binding. `:=` declares a mutable binding **and** reassigns it.
```quilon
x = 42                  ~ immutable bind (rebinding x with = is an error)
counter := 0            ~ mutable bind
counter := counter + 1  ~ reassign (also :=)
```
Reassigning requires a mutable binding: `x := 5` on an immutable `x` is an error.
Types are inferred, and may be annotated: `x :: Num = 42`.

For a record — and for containers holding records — the binding operator governs the
**value**: an `=`-bound value is immutable through every alias, and a binding that would
put one value on both sides of the `=`/`:=` line is a compile error (see
[Deep immutability](mutation.md#deep-immutability)).

## A top-level binding is a constant or a function

A binding written outside any function is a **global**. A global's initializer is a
constant: a `Num`, `Bool` or `$` literal, or a function. Mutable (`:=`) globals follow the
same rule, and a `:=` global is writable from inside a function like any other.

```quilon
limit = 10              ~ fine
enabled = true          ~ fine
scale = (n :: Num) => < n * 3 > ~ fine — a function value
counter := 4            ~ fine — and writable from a function

doubled = limit * 2     ~ error: a computed value
greeting = "hi"         ~ error: a Text is built at runtime
sizes = [1, 2]          ~ error: an array is built at runtime
origin = { x = 0 }      ~ error: a record is built at runtime
```

A rejected binding reports what it is and names the fix: the work moves into the function
that uses it. A computed binding *inside* a function is ordinary. (See
`examples/globals.qn` and `examples/global_computed.qn`.)
