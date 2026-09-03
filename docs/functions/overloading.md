---
title: "Overloading"
---

# Overloading

Quilon has **explicit ad-hoc overloading** — its polymorphism. Top-level definitions that
share a name and each annotate their parameters *are* an overload set, with no marker:

```quilon ignore
score = (n :: Num)  -> Num => < n + 1 >   ~ the Num member
score = (s :: Text) -> Num => < s.size >  ~ the Text member

a = score(41)       ~ 42  — picks the Num member
b = score("abcd")   ~ 4   — picks the Text member
```

**Dispatch is by exact static argument type, with NO implicit coercion.** No match, or two
members sharing a parameter-type list, is a compile error listing the candidates:

```text
error[QN311]: no overload of `score` takes (Bool)
  help: the members of `score` are (Num), (Text)
```

- Every member must annotate **all** its parameters **and its return type** — the signature
  is what dispatch selects on, and a call has to know what it produces:
  ```quilon ignore
  g = (n :: Num) => < 1 >    ~ error: overload member 'g' (Num) has no return type
  g = (t :: Text) -> Num => < 2 >
  ```
  Stating the whole signature on the binding annotates both at once:
  ```quilon ignore
  g :: (Num) -> Num = (n) => < 1 >
  g :: (Text) -> Num = (t) => < 2 >
  ```
- An overload set has two or more members; a single `name = …` definition is an ordinary
  function with an inferred return type. Its parameters are annotated — a **method**
  parameter is held to the same rule (see
  [named record types](../types/records.md#named-record-types-with-methods)).
- **The built-in operators are members.** `+` on `Num` and `+` on
  `Text` are two members of the `+` set, and a type's own operator member joins it on the
  same terms.
- **A module's overload sets are [closed](../modules/README.md#closed-overload-sets).**
  `io.print` / `io.write` / `time.now` are reached through their module's binding, and
  a program's own bare `print` or `write` — at any signature — is an unrelated function.
  The output built-ins take any renderable value; a type becomes printable by defining its
  [`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-),
  and a program builds on a module by wrapping it:
  ```quilon ignore
  write = (content :: Text) -> Num => < io.write(content, io.stdout) > ~ the program's own
  write("raw")                 ~ a plain call of the wrapper
  io.write("raw", io.stdout)   ~ the module's, through its binding
  ```
- A member joins its set where it is written, so a call resolves only against the members
  above it ([names resolve top to bottom](README.md#names-resolve-top-to-bottom)).
- Dispatch is resolved at **direct call sites** by static argument types. An overloaded
  name is called; a lambda that calls it is the value to pass on.

## Operator overloading

An operator is user-overloadable — `+ - * / %`, `== != < <= > >=` — as a **member of the
type it operates on** (a [record](../types/records.md#named-record-types-with-methods) or a
[sum](../types/sum-types.md)). `it` is the **left** operand. A **binary** operator member takes one
explicit parameter, the **right** operand; a unary one (the render `` ` ``) takes none.
An operator member is `=`-declared and yields a value, leaving `it` as it was
(see [Mutation](../mutation.md)):

```quilon
Vec = {
  x :: Num, y :: Num,
  + = (other :: Vec) -> Vec => < Vec { x = it.x + other.x, y = it.y + other.y } >
  == = (other :: Vec) -> Bool => < it.x == other.x && it.y == other.y >
}

v = Vec { x = 1, y = 2 } + Vec { x = 3, y = 4 }   ~ resolves to Vec's `+`
```

`a <op> b` resolves the operator from the **left operand's** type; the right operand may
be any type (`Vec * Num -> Vec`). Resolution is exact-typed like any overload, and
lowers to a direct call. The built-in operators (`Num`/`Text` `+`, `==` over any scalar,
`<`/`>`/`<=`/`>=` over `Num`/`Text`) are members of the same sets, so `"abc" < "abd"` is
defined. The definable comparison members are `<=`/`>=`, `==`/`!=`; a `<`/`>` at
member-name position reads as a block.

A **comparison/equality** member (`== != < <= > >=`) **must return `Bool`**; **arithmetic**
members (`+ - * / %`) return whatever they declare. A **top-level** operator definition is
rejected — the operator must be a member of its type.

**The `%` hash hook.** A **unary** `% = () -> Num => < … >` member (`it` the value, no explicit
parameter) is the type's **hash**, letting it be a [Map/Set key](../collections/README.md#maps) alongside its `==`
member. Both are required together, and `%`/`==` must agree: equal values hash the same.
This unary `%` is distinct from the binary `%` remainder operator, which takes one
parameter. It has no call syntax of its own — the collections invoke it.

(See `examples/overloading.qn`, `examples/sum_methods.qn`, `examples/maps.qn`,
`examples/sets.qn`, and `examples/overload_dispatch.qn` for dispatch on argument types out
of an array element, a match, a call, or a lambda.)
