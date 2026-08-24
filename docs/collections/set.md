# `Set` — unordered unique collection

A built-in, no-import type. See the [Language reference](../LANGUAGE.md#sets) and `examples/sets.ql`.

A `Set` is written `[|T|]`. `Set` is a **built-in parametric collection** — like `[]T`,
not a user-defined generic — written with the same **pipe fence** `[| … |]`; the fence is
what keeps a set literal distinct from an array (`[1, 2, 3]` is an array, `[|1, 2, 3|]` is
a set).

```quilon
primes :: [|Num|] = [|2, 3, 5, 7|]                        ~ a Set
none   :: [|Num|] = [||]                                  ~ empty set
```

**Elements** may be `Num`, `Text` (hashed **by content**, consistent with `==`), or `Bool`;
duplicates collapse. A set is **immutable / persistent**: every mutator (`add`, the set
operators) returns a **new** set and never touches the receiver.

**Iteration order is UNSPECIFIED** — conceptually a set is unordered, so never rely on the
order of `items`/`each`. (It is the hash order, *not* insertion order. A fixed-seed hasher
makes it reproducible run-to-run so example asserts don't flake, but that is an
implementation detail, not a contract.)

A set carries a built-in `.size` **field** (element count, like an array's `.size`);
everything else is a reserved method (resolved ahead of any same-named user overload when
the receiver is a Set):

| Set method | Result | Notes |
|------------|--------|-------|
| `has(x)` | `Bool` | membership |
| `add(x)` | new `[\|T\|]` | a fresh set with `x` added (persistent) |
| `items()` | `[]T` | the elements as an array (order unspecified) |
| `each(x => …)` | **the receiver set** | runs the body per element for effect, then returns the set (chains) |

**Set algebra** (each builds a new set of the same element type):

```quilon
[|1, 2, 3|] + [|3, 4, 5|]    ~ union        → {1, 2, 3, 4, 5}
[|1, 2, 3|] - [|3, 4, 5|]    ~ difference   → {1, 2}
[|1, 2, 3|] +- [|3, 4, 5|]   ~ intersection → {3}   (`+-` and `-+` are the same operator)
```

Like the empty array `[]` (which is `[]Num`), an **empty** set literal defaults to `Num`
element type and cannot yet be annotated to another type. User-defined element types (via a
`%` hash hook) and element removal are deferred (not in the initial surface).

(See `examples/sets.ql`.)
