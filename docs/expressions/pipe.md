---
title: "Pipe — |>"
sidebar:
  label: "Pipe"
  order: 1
---

# Pipe — `|>`
`|>` feeds its left operand in as the **first argument** of the right-hand call:
```quilon ignore
x |> f          ~ ≡ f(x)
x |> f(a)       ~ ≡ f(x, a)
10 |> double |> addFive   ~ ≡ addFive(double(10))
```
The result is the plain call form, so it names the top-level namespace: a
[method](../types/records.md) is reached through `recv.name(...)`, never through a pipe.

(See `examples/pipeline.qn`.)
