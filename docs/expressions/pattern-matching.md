---
title: "Pattern matching"
sidebar:
  order: 4
---

# Pattern matching

```quilon ignore
result = value ?
  | 0        => "zero"
  | 1        => "one"
  | _        => "other"      ~ wildcard
```

Every match is **total**; the type checker rejects a match with an uncovered value:

- a **sum-typed** scrutinee is covered by listing every variant (`Ok(x)` / `NotOk(e)`), or by a catch-all;
- **any other** scrutinee — a `Num`, a `Text` — is covered by a catch-all. Both `_` and a binding arm (`| rest => rest * 2`) are catch-alls.

A constructor pattern must name a variant of the scrutinee's sum type: `Purple` against a `Color` that has none, or `Ok(x)` against a `Num`, is a compile error.

A match written as a match arm's own body is parenthesized:

```quilon ignore
Mood = Grumpy(Num) / Giddy(Num)
today = (state :: Mood, luck :: Num) -> Num => < state ?
  | Grumpy(n) => (luck ? | 0 => n | _ => n * 2)
  | Giddy(n) => n
>
```

A ternary in an arm's body (`a ? b : c`) stays bare — it is a different construct, and its
`:` already marks where its branches end. A match nested inside a block, a call argument,
or a ternary branch is already delimited the same way and needs no parentheses of its own.
See [QN114](../tooling/errors.md#qn114--match-used-as-a-match-arm-body-without-parentheses).

(See `examples/pattern_match.qn`.)
