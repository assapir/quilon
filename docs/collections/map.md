---
title: "Map — keyed collection"
sidebar:
  label: "Map"
---

# `Map` — keyed collection

A built-in, no-import type. See the [Language reference](README.md#maps) and `examples/maps.qn`.

A `Map` is written `[|K => V|]` (`=>` reads "maps to"). `Map` is a **built-in parametric
collection**, like `[]T`, written with a **pipe fence** `[| … |]`.

```quilon
ages :: [|Text => Num|] = [|"ada" => 36, "alan" => 41|]   ~ a Map
empty :: [|Num => Num|] = [|=>|]                          ~ empty map
```

**Keys** may be `Num`, `Text` (hashed **by content**, consistent with `==`), `Bool`, or a
**user type that opts in** (see below). `set` and `remove` are **setters**: they mutate a
`:=`-bound map in place and return `it`, so calls chain like `each` (see
[mutation](../mutation.md)). Calling either on an `=`-bound map is a compile error.

A map literal names each literal key once: `[|"a" => 1, "a" => 2|]` is a duplicate
definition. Only the literal token is compared — two computed keys that happen to be
equal are unaffected: the later entry wins, as a `set` of the same key would.

## User-defined key types

A record or sum type becomes a key by defining two members (see
[operator members](../functions/overloading.md#operator-overloading)):

- a **`%` hash hook** — a unary member `% = () -> Num => < … >` (`it` is the value) returning a
  `Num` hash;
- an **`==` member** — the usual equality, `== = (other :: T) -> Bool => < … >`.

Both are required: a type used as a key with only one is a compile error. Keys are hashed
and compared through these members, so `%` and `==` must agree — two keys that are `==`
must return the same `%` (debug builds check this and fail loud on a violation). The `%`
hook has no call syntax of its own; the collections invoke it.

```quilon
Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >,
  % = () -> Num => < it.x * 31 + it.y >
}
grid :: [|Point => Text|] = [|Point { x = 0, y = 0 } => "origin"|]
```

**Iteration order is UNSPECIFIED** — a map is unordered, and the order of
`keys`/`values`/`each` is an implementation detail that may change between runs.

**Access is via `.get`, which returns a `Result`** — `Ok(value)` when the key is present,
`NotOk` when it is absent — and a caller matches both cases. Bracket indexing is defined on
arrays; `m[k]` on a map is a type error.

A map carries a built-in `.size` **field** (entry count, like an array's `.size`);
everything else is a reserved method (resolved ahead of any same-named user overload when
the receiver is a Map):

| Map method | Result | Notes |
|------------|--------|-------|
| `get(k)` | `Ok(v)` / `NotOk` | the safe, `Result`-returning lookup (the only way to read a value) |
| `has(k)` | `Bool` | membership |
| `set(k, v)` | **the receiver map** | binds `k` to `v` in place; requires a `:=`-bound receiver |
| `remove(k)` | **the receiver map** | removes `k` in place; requires a `:=`-bound receiver; removing an absent key is a no-op |
| `keys()` | `[]K` | the keys as an array (order unspecified) |
| `values()` | `[]V` | the values as an array (same order as `keys()`) |
| `each((k, v) => …)` | **the receiver map** | runs the body per entry for effect, then returns the map (chains); visits the entries present when it starts — a body that mutates the receiver changes the map, not the walk |

```quilon ignore
<< core.io
m :: [|Text => Num|] = [|"a" => 1, "b" => 2|]
total = m.values().reduce(0, (acc, x) => acc + x)   ~ 3
m.get("a") ? | Ok(v) => v | NotOk(_) => 0           ~ 1
```

An **empty** map literal `[|=>|]` has no key/value type of its own — it takes one from
context: a type annotation on the binding, a call argument's declared parameter type, or a
function's declared return type. With none of those available, it's a compile error (there
is no `Num` default).

(See `examples/maps.qn`.)
