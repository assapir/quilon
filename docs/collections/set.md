---
title: "Set — unordered unique collection"
sidebar:
  label: "Set"
---

# `Set` — unordered unique collection

A built-in, no-import type. See the [Language reference](README.md#sets) and `examples/sets.qn`.

A `Set` is written `[|T|]`. `Set` is a **built-in parametric collection**, like `[]T`,
written with the same **pipe fence** `[| … |]`; the fence distinguishes a set literal
from an array (`[1, 2, 3]` is an array, `[|1, 2, 3|]` is a set).

```quilon
primes :: [|Num|] = [|2, 3, 5, 7|]                        ~ a Set
none   :: [|Num|] = [||]                                  ~ empty set
```

**Elements** may be `Num`, `Text` (hashed **by content**, consistent with `==`), `Bool`, or
a **user type that opts in** — a record or sum defining both a `%` hash hook
(`% = () -> Num => < … >`, `it` the value) and an `==` member; both are required, and `%`/`==`
agree (see [Map](map.md#user-defined-key-types)). Duplicates collapse. `add` and `remove` are
**setters**: they mutate a `:=`-bound set in place and return `it`, so calls chain like
`each` (see [mutation](../mutation.md)). Calling either on an `=`-bound set is a compile
error. The set operators `+`/`-`/`+-` stay non-mutating and build a **new** set.

**Iteration order is UNSPECIFIED** — a set is unordered, and the order of `items`/`each`
is an implementation detail that may change between runs.

A set carries a built-in `.size` **field** (element count, like an array's `.size`);
everything else is a reserved method (resolved ahead of any same-named user overload when
the receiver is a Set):

| Set method | Result | Notes |
|------------|--------|-------|
| `has(x)` | `Bool` | membership |
| `add(x)` | **the receiver set** | adds `x` in place; requires a `:=`-bound receiver |
| `remove(x)` | **the receiver set** | removes `x` in place; requires a `:=`-bound receiver; removing an absent element is a no-op |
| `items()` | `[]T` | the elements as an array (order unspecified) |
| `each(x => …)` | **the receiver set** | runs the body per element for effect, then returns the set (chains) |

**Set algebra** (each builds a new set of the same element type):

```quilon
[|1, 2, 3|] + [|3, 4, 5|]    ~ union        → {1, 2, 3, 4, 5}
[|1, 2, 3|] - [|3, 4, 5|]    ~ difference   → {1, 2}
[|1, 2, 3|] +- [|3, 4, 5|]   ~ intersection → {3}   (`+-` and `-+` are the same operator)
```

An **empty** set literal `[||]` has no element type of its own — it takes one from context:
a type annotation on the binding, a call argument's declared parameter type, or a
function's declared return type. With none of those available, it's a compile error (there
is no `Num` default).

(See `examples/sets.qn`.)
