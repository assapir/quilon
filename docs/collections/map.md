---
title: "Map — keyed collection"
sidebar:
  label: "Map"
---

# `Map` — keyed collection

A built-in, no-import type. See the [Language reference](README.md#maps) and `examples/maps.qn`.

A `Map` is written `[|K => V|]` (`=>` reads "maps to"). `Map` is a **built-in parametric
collection** — like `[]T`, not a user-defined generic — written with a **pipe fence**
`[| … |]`.

```quilon
ages :: [|Text => Num|] = [|"ada" => 36, "alan" => 41|]   ~ a Map
empty :: [|Num => Num|] = [|=>|]                          ~ empty map
```

**Keys** may be `Num`, `Text` (hashed **by content**, consistent with `==`), `Bool`, or a
**user type that opts in** (see below). A map is **immutable / persistent**: every mutator
(`set`) returns a **new** map and never touches the receiver.

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

**Iteration order is UNSPECIFIED** — conceptually a map is unordered, so never rely on the
order of `keys`/`values`/`each`. (It is *not* insertion order. It may look stable
run-to-run; that is not a contract.)

**Access is via `.get`, which returns a `Result`** — `Ok(value)` when the key is present,
`NotOk` when it is absent — so a caller must handle the missing case. There is **no bracket
indexing on a map** (`m[k]` is a type error; bracket indexing is arrays only).

A map carries a built-in `.size` **field** (entry count, like an array's `.size`);
everything else is a reserved method (resolved ahead of any same-named user overload when
the receiver is a Map):

| Map method | Result | Notes |
|------------|--------|-------|
| `get(k)` | `Ok(v)` / `NotOk` | the safe, `Result`-returning lookup (the only way to read a value) |
| `has(k)` | `Bool` | membership |
| `set(k, v)` | new `[\|K => V\|]` | a fresh map with `k` bound to `v` (persistent) |
| `remove(k)` | new `[\|K => V\|]` | a fresh map without `k` (persistent); removing an absent key is a no-op |
| `keys()` | `[]K` | the keys as an array (order unspecified) |
| `values()` | `[]V` | the values as an array (same order as `keys()`) |
| `each((k, v) => …)` | **the receiver map** | runs the body per entry for effect, then returns the map (chains) |

```quilon ignore
<< core.io
m :: [|Text => Num|] = [|"a" => 1, "b" => 2|]
total = m.values().reduce(0, (acc, x) => acc + x)   ~ 3
m.get("a") ? | Ok(v) => v | NotOk(_) => 0           ~ 1
```

Like the empty array `[]` (which is `[]Num`), an **empty** map literal defaults to `Num`
key/value types and cannot yet be annotated to another type.

(See `examples/maps.qn`.)
