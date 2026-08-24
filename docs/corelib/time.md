# `core.time` — Time

Import with `<< core.time`. See the [corelib index](../LANGUAGE.md#corelib) and `examples/sleep.qn`.

The [`@sleep`](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) leaf IO primitive (a pause) and the monotonic `now()` clock.

| Function | Effect |
|----------|--------|
| `@sleep(seconds :: Num) -> $` | Pause the current fiber for `seconds` (a fractional `Num`); execution then continues in program order. Effect-only, so nothing defers or forces. A [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) (the `@` marker). |
| `now() -> Num` | Read a **monotonic** clock (seconds, fractional `Num`). Only *differences* between readings are meaningful, so it measures elapsed time. A plain (non-`@`) primitive: instant, never parks. |

`now` is an [overload set](../LANGUAGE.md#overloading) — defining it with another signature
adds a member rather than shadowing the clock.

```quilon
<< core.time

^ = () -> Num => <
  start = now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep really waited → 42
>
```

Only the leaf `@sleep` is marked — `^` and any helper it calls carry nothing. See the
[Concurrency model](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) and
`examples/sleep.qn`.
