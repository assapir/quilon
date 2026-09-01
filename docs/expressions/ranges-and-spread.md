---
title: "Ranges and spread"
sidebar:
  order: 3
---

# Ranges and spread

## Ranges — infix `lo <- hi`
The infix `<-` operator builds an **inclusive** `[]Num`:
```quilon
1 <- 4          ~ [1, 2, 3, 4]
4 <- 1          ~ [4, 3, 2, 1]   (descends when the left end is larger)
5 <- 5          ~ [5]            (single point)
```
It is pure **array sugar**: there is no distinct `Range` type. The result *is* a
`[]Num`, so it composes with `.size`, indexing `[i]`, and the [array methods](../collections/arrays.md#array-methods):
```quilon
r = 2 <- 5      ~ [2, 3, 4, 5]
n = r.size      ~ 4   (inclusive count = |hi - lo| + 1)
first = r[0]    ~ 2
r.each(x => io.print(x))   ~ a range iterates with `.each` like any array
```
Both ends are full `Num` expressions — they may be dynamic, not just literals. The
direction (ascending vs descending) is decided at runtime. (See `examples/ranges.qn`.)

### Endpoints must be whole numbers
A range counts from one end to the other, so each end must be a **whole number** — and one a
`Num` holds exactly, so at most 2^53 in magnitude (the
[exact-integer limit](../types/README.md#the-exact-integer-limit)). Anything else is an
**error**, never a truncation (a range is also
[materialized in full](../status/limitations.md)):

```quilon ignore
1.5 <- 3.9        ~ error: a range endpoint must be a whole number (got 1.5)
1 <- (0.0 / 0.0)  ~ error: a range endpoint must be a whole number (got NaN)
1 <- 100000000000000000
~ error: a range endpoint must be a whole number a Num holds exactly, at most
~        9007199254740992 in magnitude (got 100000000000000000)
```

What the compiler can evaluate it rejects at compile time; anything computed is rejected
when the range runs, framed at the range expression and exiting 1 — the same fail-loud
contract a bad [`array[i]`](../collections/arrays.md) has.

## Spread in literals
The **prefix** `<-` splices a source's contents into an array or record literal:

- **Array spread** `[<-xs, 4, 5]` builds a new array of every element of `xs`, then `4, 5`.
  Multiple spreads apply left-to-right (`[0, <-a, <-b, 9]`). The source must be an array of
  the literal's element type; `[]Text`, `[]Num`, and nested arrays all splice. `[<-xs]` alone
  copies `xs`.
- **Record functional-update** `{<-p, x = 9}` builds a new record copying every field of
  `p`, then applying the overrides. Later entries override earlier ones (left-to-right),
  and an entry naming a field not in `p` **adds** it. If `p` is a **named** record and the
  result reproduces that type's fields exactly (only overriding existing fields, adding
  nothing), the result keeps the **named type and its methods**; otherwise it is an
  anonymous record.
- **Naming the type you are building** — `Vec {<-p, x = 9}` — is the same update as a
  constructor. The stated target constrains the source: it must be **already that type** or
  an **anonymous record of exactly its shape** (same fields and types, nothing extra). A
  different named type is never accepted, however similar — `Point` and `Other` stay
  distinct. An anonymous record cannot fill a type declaring **methods**. Every declared
  field must end up provided, by the spread or an override.

```quilon
xs = [1, 2, 3]
ys = [<-xs, 4, 5]        ~ [1, 2, 3, 4, 5]
zs = [0, <-xs, <-ys]     ~ [0, 1,2,3, 1,2,3,4,5]

Vec = { x :: Num, y :: Num, sum = => < it.x + it.y >}
a = Vec { x = 10, y = 20 }
b = { <-a, x = 5 }       ~ still a Vec: b.sum() → 25
c = Vec { <-a, x = 5 }   ~ the same update, naming the type being built
```

**Range vs. spread.** `<-` is both the infix inclusive range (`lo <- hi`) and the prefix
spread. **Position** tells them apart: as the first token of a `[ ]` element or `{ }`
field it is a spread; after a complete expression it is the range. So:

- `[1 <- 4]` is a **one-element** array whose sole element is the range `[1,2,3,4]`
  (the `<-` follows the complete expression `1`).
- `[<-xs, 4]` **spreads** `xs` (the `<-` begins the element).
- Inside a spread the source is a full expression, so `[<-1 <- 4]` spreads the range
  `1 <- 4` — i.e. `[1, 2, 3, 4]`.

(See `examples/spread.qn`.)
