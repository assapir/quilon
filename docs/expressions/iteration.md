---
title: "Iteration — array methods + recursion"
sidebar:
  label: "Iteration"
  order: 2
---

# Iteration — array methods + recursion
Quilon has **no `for`/`while` loop**. A collection is iterated with the built-in
[array methods](../collections/arrays.md#array-methods): `.each` runs a body for its side effects (the direct
replacement for a side-effecting loop), and `.map`/`.filter`/`.reduce` transform or fold
without any mutable accumulator. Each takes a lambda, applied per element:
```quilon
nums = [1, 2, 3]
nums.each(n => io.print(n))              ~ side effects; returns the receiver (chainable)

sum = nums
  .map(n => n * 2)                    ~ [2, 4, 6]
  .reduce(0, (acc, n) => acc + n)     ~ 12
```
Iteration beyond the methods is **recursion**: a self-tail-call
[runs in constant stack](../functions/closures.md#tail-self-recursion-runs-in-constant-stack-guaranteed),
at any depth. (See `examples/iteration.qn`.)
