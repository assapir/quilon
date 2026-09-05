---
title: "Arrays — []T"
sidebar:
  label: "Arrays"
---

# Arrays — `[]T`
```quilon
nums  = [1, 2, 3, 4, 5]
count = nums.size      ~ → 5
first = nums[0]        ~ → 1
```
(See `examples/arrays.qn`.)

A `:=`-bound array supports an in-place element write, `arr[i] := value` (the analog of
`obj.field := value` for a record; see [mutation](../mutation.md)). On an `=`-bound array
it is a compile error. Every other operation (`+`, the array methods) builds a **new**
array, leaving its operand(s) as they were. A `:=` binding may also be rebound to a
different array — that changes the binding.

```quilon
nums := [1, 2, 3, 4, 5]
nums[0] := 10          ~ [10, 2, 3, 4, 5]
```

An **empty** array literal `[]` takes its element type from context: a type annotation on
the binding, a call argument's declared parameter type, or a function's declared return
type. With none of those available, it is a compile error.

Indexing is **checked**, for a read (`arr[i]`) and an element write (`arr[i] := value`)
alike. An out-of-bounds, negative, or NaN index is a runtime error naming the read/write
that failed ([shape](../tooling/errors.md)), with exit status 1. A **fractional** in-range
index truncates toward zero: `nums[1.7]` reads `nums[1]`, so index arithmetic like
`size / 2` indexes directly. For an index that may be out of range,
[`at(n)`](#array-methods) is the `Ok`/`NotOk` form — see the computed-index case at the
end of `examples/array_methods.qn`.

## Array methods

Arrays carry a set of **built-in, compiler-provided methods**, called with method
syntax (`array.method(...)`) and freely chainable. The higher-order ones take a **lambda**
(`x => …`, `(a, b) => …`) written as a direct argument to the method.

| Method | Result | Notes |
|--------|--------|-------|
| `map(f)` | new `[]R` | element type `R` is `f`'s return type (so `map` may change the element type, e.g. `[]Num → []Text`) |
| `filter(predicate)` | new `[]element` | keeps the elements where `predicate` returns `Bool` `true`, in order; `predicate` **must** return `Bool` |
| `reduce(initial, (accumulator, x) => …)` | the accumulator | fold-left from `initial`; the reducer's result type must match `initial`'s type |
| `each(f)` | **the receiver array** | runs `f` for side effects, then returns the array itself, so it chains; walks indices `0` to `size - 1` over the array's own storage, so a body writing `arr[j] := v` ahead of the walk is read back the moment the walk reaches `j` |
| `find(predicate)` | `Ok(element)` / `NotOk` | the first element satisfying `predicate`, absent-safe; `predicate` returns `Bool` |
| `at(n :: Num)` | `Ok(element)` / `NotOk` | checked index as a value — `Ok` in bounds, `NotOk` otherwise (NaN included); a raw `array[n]` out of bounds is a runtime error |

```quilon
nums = [1, 2, 3, 4, 5, 6]

total = nums
  .map(x => x * 2)              ~ [2, 4, 6, 8, 10, 12]
  .filter(x => x > 4)           ~ [6, 8, 10, 12]
  .reduce(0, (acc, x) => acc + x)   ~ 36

first = nums.find(x => x > 3) ?  ~ Ok(4)
  | Ok(v)    => v
  | NotOk(_) => 0

third = nums.at(2) ?             ~ Ok(3)
  | Ok(v)    => v
  | NotOk(_) => 0
```

These methods are **reserved on arrays**: on an *array receiver* the built-in wins over a
same-named user function or overload (e.g. a `map` on a `Num`) — it is resolved ahead of
the overload set. `map`/`reduce`/`find` work over every element type (`[]Text` as much as
`[]Num`). (See `examples/array_methods.qn`.)

## Array concatenation — `+`

`+` on arrays builds a **new** array, leaving both operands as they were. It has three
forms, and the **exact** operand types select the form:

```quilon
~ concat:  []T + []T -> []T
[1, 2] + [3, 4]          ~ [1, 2, 3, 4]
["a"] + ["b", "c"]       ~ ["a", "b", "c"]

~ append:  []T + T   -> []T   (add one element at the end)
[1, 2] + 3               ~ [1, 2, 3]
["a"] + "b"              ~ ["a", "b"]

~ prepend: T   + []T -> []T   (add one element at the front)
0 + [1, 2]               ~ [0, 1, 2]
```

Both sides agree on the element type — `[]Num + []Text` (or `[]Num + Text`) is a type
error. The forms are mutually exclusive: an array `[]T` is a distinct type from its element
`T`. Nested arrays follow the same rule: `[][]Num + []Num` is an **append** (the `[]Num` is
a single new row → `[][]Num`), and `[][]Num + [][]Num` is a **concat**. `[]T + []T` is the
same as the spread `[<-a, <-b]`, for `[]Num`, `[]Text`, and nested arrays alike. (See
`examples/array_concat.qn`.)
