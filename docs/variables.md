---
title: "Variables"
---

# Variables

Bindings are immutable by default (`=`). Use `:=` to declare a mutable binding **and** to reassign it.
```quilon
x = 42                  ~ immutable bind (rebinding x with = is an error)
counter := 0            ~ mutable bind
counter := counter + 1  ~ reassign (also :=)
```
Reassigning requires the binding to be mutable: `x := 5` on an immutable `x` is an error.
Types are inferred but can be annotated: `x :: Num = 42`.

## A top-level binding must be a constant or a function

A binding written outside any function is a **global**. A global's initializer has to be
a constant already: no Quilon code runs before `^`, so there is nowhere to compute one.
The value may be a `Num`, `Bool` or `$` literal, or a function. Mutable (`:=`) globals
follow the same rule, and a `:=` global is writable from inside a function like any other.

```quilon
limit = 10              ~ fine
enabled = true          ~ fine
scale = (n :: Num) => n * 3   ~ fine — a function value
counter := 4            ~ fine — and writable from a function

doubled = limit * 2     ~ error: has to be computed
greeting = "hi"         ~ error: a Text is built at runtime
sizes = [1, 2]          ~ error: an array is built at runtime
origin = { x = 0 }      ~ error: so is a record
```

A rejected binding reports what it is and how to fix it — move the work into the function
that uses it. Anything computed is perfectly ordinary *inside* a function; the restriction
is only about globals. (See `examples/globals.qn` and `examples/global_computed.qn`.)
