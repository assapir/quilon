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
  bump := (by :: Num) => it.value := it.value + by  ~ may mutate `it`
  peek = => it.value                                ~ promises not to
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
~ error: Method 'Counter.bumpAll' mutates 'it' but is declared with '='
bumpAll = (steps :: []Num) => steps.each(s => it.value := s)
```

A setter call requires a `:=` receiver:

```quilon ignore
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

**Setters live on records.** Only a record's named methods may be declared `:=`. A sum's
methods, and operator members on either kind (`` ` ``, `==`, `+`, …), are always `=` and
non-mutating; `:=` on one is a compile error. Nothing they can do mutates the receiver
anyway: a sum keeps its data in variant payloads, whose match bindings are immutable, and
an operator or render member yields a value.

```quilon ignore
Shape = Circle(Num) / Rect(Num, Num) {
  area := () -> Num => 0      ~ error: a sum cannot have a mutating method
}

Counter = {
  value :: Num,
  + := (other :: Counter) -> Num => it.value   ~ error: an operator member is never `:=`
}
```

(See `examples/mutation.qn`.)
