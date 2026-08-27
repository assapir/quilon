---
title: "Pipe — |>"
sidebar:
  order: 1
---

# Pipe — `|>`
`|>` feeds its left operand in as the **first argument** of the right-hand call:
```quilon ignore
x |> f          ~ ≡ f(x)
x |> f(a)       ~ ≡ f(x, a)
10 |> double |> addFive   ~ ≡ addFive(double(10))
```
(See `examples/pipeline.qn`.)
