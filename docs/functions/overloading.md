---
title: "Overloading"
---

# Overloading

Quilon has **explicit ad-hoc overloading** — the only polymorphism, since there are no
generics. Top-level definitions that share a name and each annotate their parameters *are*
an overload set; there is no marker:

```quilon ignore
score = (n :: Num)  -> Num => n + 1       ~ the Num member
score = (s :: Text) -> Num => s.size      ~ the Text member

a = score(41)       ~ 42  — picks the Num member
b = score("abcd")   ~ 4   — picks the Text member
```

**Dispatch is by exact static argument type, with NO implicit coercion.** No match, or two
members sharing a parameter-type list, is a compile error listing the candidates:

```
error: No overload of 'score' matches argument types (Bool). Candidates: (Num), (Text)
```

- Every member must annotate **all** its parameters **and its return type** — the signature
  is what dispatch selects on, and a call has to know what it produces:
  ```quilon ignore
  g = (n :: Num) => 1        ~ error: overload member 'g' (Num) has no return type
  g = (t :: Text) -> Num => 2
  ```
  Stating the whole signature on the binding annotates both at once:
  ```quilon ignore
  g :: (Num) -> Num = (n) => 1
  g :: (Text) -> Num = (t) => 2
  ```
- A single ordinary `name = …` definition is **not** an overload set. It keeps its
  inferred return type — no return annotation needed. Its parameters are still annotated;
  only an unannotated **method** parameter defaults to `Num` (see
  [named record types](../types/records.md#named-record-types-with-methods)).
- **The compiler's own definitions are members, not reserved names.** The built-in
  operators, and the corelib functions the compiler provides (`print`/`eprint`, `write`,
  `now`), are members of their sets like any other. Defining one of those names with a
  different signature ADDS a member that wins for its argument types; the built-in
  stays reachable for the types it claims. Defining the built-in's own signature is the
  usual duplicate-definition error:
  ```quilon ignore
  write = (content :: Text) -> Num => write(content, stdout)  ~ adds a member…
  write("raw")           ~ …which this call picks
  write("raw", stdout)   ~ while this one still reaches the built-in
  ```
- A member joins its set where it is written, so a call resolves only against the members
  above it ([names resolve top to bottom](README.md#names-resolve-top-to-bottom)).
- Dispatch is resolved at **direct call sites** by static argument types. Passing an
  overloaded name as a value (higher-order use) is not yet supported.

## Operator overloading

An operator is user-overloadable — `+ - * / %`, `== != < <= > >=` — as a **member of the
type it operates on** (a [record](../types/records.md#named-record-types-with-methods) or a
[sum](../types/sum-types.md)). `it` is the **left** operand. A **binary** operator member takes one
explicit parameter, the **right** operand; a unary one (the render `` ` ``) takes none.
An operator member is always `=`-declared and yields a value; it never mutates `it`
(see [Mutation](../mutation.md)):

```quilon
Vec = {
  x :: Num, y :: Num,
  + = (other :: Vec) -> Vec => Vec { x = it.x + other.x, y = it.y + other.y }
  == = (other :: Vec) -> Bool => it.x == other.x && it.y == other.y
}

v = Vec { x = 1, y = 2 } + Vec { x = 3, y = 4 }   ~ resolves to Vec's `+`
```

`a <op> b` resolves the operator from the **left operand's** type; the right operand need
not be the same type (`Vec * Num -> Vec`). Resolution is exact-typed like any overload, and
lowers to a direct call. The built-in operators (`Num`/`Text` `+`, `==` over any scalar,
`<`/`>`/`<=`/`>=` over `Num`/`Text`) are members of the same sets, so `"abc" < "abd"` works
out of the box. (`<`/`>` are not definable as members — a `<`/`>` at member-name position
would read as a block; use `<=`/`>=`.)

A **comparison/equality** member (`== != < <= > >=`) **must return `Bool`**; **arithmetic**
members (`+ - * / %`) return whatever they declare. A **top-level** operator definition is
rejected — the operator must be a member of its type.

**The `%` hash hook.** A **unary** `% = () -> Num => …` member (`it` the value, no explicit
parameter) is the type's **hash**, letting it be a [Map/Set key](../collections/README.md#maps) alongside its `==`
member. Both are required together, and `%`/`==` must agree: equal values hash the same.
This unary `%` is distinct from the binary `%` remainder operator, which takes one
parameter. It has no call syntax of its own — the collections invoke it.

(See `examples/overloading.qn`, `examples/sum_methods.qn`, `examples/maps.qn`,
`examples/sets.qn`, and `examples/overload_dispatch.qn` for dispatch on argument types out
of an array element, a match, a call, or a lambda.)
