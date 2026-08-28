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

Every match is **total** — the type checker rejects one that isn't:

- a **sum-typed** scrutinee is covered by listing every variant (`Ok(x)` / `NotOk(e)`), or by a catch-all;
- **any other** scrutinee needs a catch-all, since nothing enumerates the values of a `Num` or a `Text`. Both `_` and a binding arm (`| rest => rest * 2`) are catch-alls.

A constructor pattern must name a variant of the scrutinee's sum type: `Purple` against a `Color` that has none, or `Ok(x)` against a `Num`, is a compile error.

(See `examples/pattern_match.qn`.)
