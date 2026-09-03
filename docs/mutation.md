---
title: "Mutation: in-place field writes & setters"
sidebar:
  label: "Mutation"
---

# Mutation: in-place field writes & setters

The binding operator decides mutability. It governs in-place mutation as well as
reassignment:

- An `=`-bound instance is **immutable**: no field writes, and calling a setter method on it
  is a compile error.
- A `:=`-bound instance is **mutable**: a direct field write `obj.field := value` (in place,
  no re-allocation) and any **setter** method.
- One exception is by type rather than by binding: a [`Site`](functions/site.md) is
  read-only. A location is a value, not a variable, so writing one of its fields is an
  error even through a `:=` binding.

A method is a **setter** when it is **declared** with `:=`. The binding operator is
the marker, exactly as it is for a variable — a method's right to mutate is part of its
signature.

```quilon
Counter = {
  value :: Num,
  bump := (by :: Num) => < it.value := it.value + by > ~ may mutate `it`
  peek = => < it.value >                            ~ promises not to
}

c := Counter { value = 30 }   ~ `:=` -> mutable
c.bump(5)                      ~ setter mutates in place -> value = 35
c.value := c.value + 7         ~ direct field write    -> value = 42
```

An `=` method is **held to its promise**: writing `it.field := …` in one, or calling a
`:=` sibling on `it`, is a compile error telling you to declare it `:=`. The write counts
**wherever it appears in the body** — nested inside a lambda, an array or record literal,
a match arm, an argument list, or a function declared inside the body. Nesting does not
launder a mutation:

```quilon ignore
~ error: method `Counter.bumpAll` mutates `it` but is declared with `=`
bumpAll = (steps :: []Num) => < steps.each(s => it.value := s) >
```

A setter call requires a `:=` receiver:

```quilon ignore
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

## Deep immutability

`=` freezes the **value**. A value reached through an `=` binding is never reachable
through a `:=` binding, in either direction. The rule covers the values with reference
semantics — records, and the containers that hold them (arrays, maps, sets, and sum
payloads). `Num`, `Bool`, and `Text` copy on binding and bind either way.

**Bindings.** A `:=` binding takes a fresh value, or a value reached only through `:=`
bindings; an `=` binding takes a fresh value, or a value reached only through `=`
bindings. A binding that crosses the line is a compile error at the binding:

```quilon ignore
t = T { v = 1 }    ~ `=` -> the value is frozen
a := t             ~ error: `t` is immutable
b = t              ~ legal: a second frozen name for the same value

m := T { v = 1 }   ~ `:=` -> the value is mutable
c = m              ~ error: `m` is mutable — writes through `m` would change `c`
```

**Containers.** A store across the line is a compile error at the store, in both
directions, and what is read out of an `=`-bound container is immutable:

```quilon ignore
box := Box { item = t }   ~ error: `t` is immutable
arr := [t]                ~ error: `t` is immutable
open = Box { item = m }   ~ error: `m` is mutable
x := frozenBox.item       ~ error: what a frozen container holds is frozen
```

**Escaping results.** A method may return `it`, or a value holding it. The method stays
callable on every receiver, and the call's **result inherits the receiver's mutability**
at each call site: immutable on an `=` receiver, mutable on a `:=` receiver. A function
returning a parameter follows the same rule with its argument. The rule holds however
the value travels — through a local, a container, or a sum payload:

```quilon ignore
T = { v :: Num, self = () -> T => < it > }
t = T { v = 1 }
x = t.self()       ~ legal: the result is immutable, like its receiver
y := t.self()      ~ error: `t` is immutable

m := T { v = 1 }
z := m.self()      ~ legal: the result is mutable, like its receiver
z.v := 5           ~ writes m.v
```

**Parameters.** A parameter's value belongs to the caller. A function body binds a
parameter — and an `=` method binds `it` — with `=` only; a `:=` binding of one is a
compile error at the binding. A setter's `it` is mutable, its aliases included.

**Fresh values.** A value built inside a function and returned is fresh: the function's
locals — `:=` locals included — end at the return, and each call site binds the result
with either operator.

**Write sites.** A field write and a setter call are checked against the value their
path reaches, however it is reached — a field read, an element read, or a call result
(`same(t).v := 5` on `=`-bound `t` is a compile error naming `t`).

(See `examples/deep_immutability.qn`.)

**Setters live on records.** Only a record's named methods may be declared `:=`. A sum's
methods, and operator members on either kind (`` ` ``, `==`, `+`, …), are always `=` and
non-mutating; `:=` on one is a compile error. Nothing they can do mutates the receiver
anyway: a sum keeps its data in variant payloads, whose match bindings are immutable, and
an operator or render member yields a value.

```quilon ignore
Shape = Circle(Num) / Rect(Num, Num) {
  area := () -> Num => < 0 >  ~ error: a sum cannot have a mutating method
}

Counter = {
  value :: Num,
  + := (other :: Counter) -> Num => < it.value > ~ error: an operator member is never `:=`
}
```

(See `examples/mutation.qn`.)
