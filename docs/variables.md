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

## Names

A name starts with a letter — any Unicode letter, ASCII or not — or `_`, and continues with
letters, digits and `_`. Names are case-sensitive, with no normalization: two different
spellings of the same accented letter are two different names.

```quilon
ףסא = 12  ~ a non-ASCII name, bound like any other — right-to-left included
```

A bare `_` is the wildcard pattern, never an ordinary name. `true` and `false` are the
lexer's only two reserved words; every other word lexes as an ordinary name, and the
checker reserves a handful of those (see the [symbol table](README.md#symbols)).

## A top-level binding is computed before `^`

A binding written outside any function is a **global**. Every global's initializer runs
once, before `^`, in file order — an imported module's globals run first, then the
importer's own, top to bottom. An initializer sees every name defined above it in that
order and none below it, the same no-hoisting rule an ordinary binding follows.

```quilon
limit = 10                       ~ a literal
scale = (n :: Num) => < n * 3 >  ~ a function value
doubled = scale(limit)           ~ computed from what is above
counter := 0                     ~ a mutable cell, writable from a function
```

A `:=` global is **one cell for the whole program**: every function's write to it lands in
that cell, and the next read — from any function, on any later call — sees it.

```quilon
counter := 0
countSheep = () -> Num => < counter := counter + 1; counter >
~ countSheep() then countSheep() returns 2, not 1 twice
```

`>>` on a `:=` global is a compile error (`QN346`): mutation does not cross a module
boundary. A function that reads or writes the cell is the export.

```quilon ignore
>> counter := 0   ~ error[QN346]: mutable global exported
```

(See `examples/globals.qn` and `examples/global_computed.qn`.)
