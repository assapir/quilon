---
title: "core.time — Time"
sidebar:
  label: "core.time"
  order: 4
---

# `core.time` — Time

Import with `<< core.time`. See the [corelib index](README.md) and `examples/sleep.qn`.

The [`@sleep`](../concurrency/README.md) leaf IO primitive (a pause) and the monotonic `time.now()` clock.

| Function | Effect |
|----------|--------|
| `@sleep(seconds :: Num) -> $` | Pause the current fiber for `seconds` (a fractional `Num`); execution then continues in program order. Effect-only: it carries no value to defer or force. A [leaf IO primitive](../concurrency/README.md) (the `@` marker). |
| `time.now() -> Num` | Read a **monotonic** clock (seconds, fractional `Num`). *Differences* between readings measure elapsed time. A plain (non-`@`) primitive: instant. |

`now` is an [overload set](../functions/overloading.md) — defining it with another signature
adds a member beside the clock.

```quilon
<< core.time

^ = () -> Num => <
  start = time.now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  time.now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep waited → 42
>
```

The leaf `@sleep` carries the marker; `^` and any helper it calls are unmarked. See the
[Concurrency model](../concurrency/README.md) and
`examples/sleep.qn`.
