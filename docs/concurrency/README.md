---
title: "Concurrency — colorless implicit futures (🚧 in progress)"
sidebar:
  label: "Concurrency"
---

# Concurrency — colorless implicit futures (🚧 in progress)

> Colorless implicit futures on cooperative fibers: IO returns type-invisible deferreds, strict operations force them — concurrency follows data dependence.

> **Status: 🚧 in progress.** The model below is locked. Implemented: the
> single-threaded fiber scheduler, the effect-only `@sleep` pause (`core.time`), the
> deferred-value `@readStdin` (`core.io`), and the networked `@tcpRequest` (`core.net`).
> Planned: a value-returning network primitive such as `@get`, with which two independent
> reads finish in max-time; and the multicore (M:N) runtime.

Quilon's concurrency is **colorless**: a program is written as ordinary, blocking-*looking*
code, and the runtime overlaps independent IO. A function that does IO is written and typed
exactly like one that does none; the program contains no `async`, `await`, `go`/`spawn`, or
resolve token. The model is **promise pipelining**.

**`@` marks leaf IO primitives only** — the corelib/runtime primitives that perform IO
(`http.get`, a file read, a socket receive, `sleep`). All user code is unmarked: a function that
transitively calls an `@` primitive is concurrency-capable, with **no propagation** up the
call chain.

**Deferred values.** Calling an `@` primitive launches the IO and returns immediately with a
*deferred* value; the caller continues. Deferred-ness propagates as the value flows —
passed as an argument, stored in a record or array, returned from a function — forcing
nothing along the way. That threading is the *pipelining*.

**Forcing happens at the leaves.** A deferred value is forced — the fiber parks until it is
ready — at a **strict** operation: arithmetic, comparison, pattern match (`?`), IO
output (`print`/`write`), and native calls. Values launched before they are forced overlap.

**Deferral is type-invisible.** A deferred `Text` types as `Text`, and exact-type
[overload resolution](../functions/overloading.md) sees `Text`.

**Structured & scoped.** Deferred tasks are scoped to their enclosing `< >` block: the block
forces and joins everything it launched before returning, and a panic propagates out.

**Stackful fibers.** Each fiber has its own stack, and any function parks at a force point
as it is.

**Determinism.** Pure results are deterministic. The **ordering of side effects** across
independent deferred IO is unspecified.

**A program's entry runs on the fiber scheduler**, so every `@` primitive it reaches
has a fiber to park on. A pure program pays the scheduler's fixed start-up — a reactor and one
fiber stack — and the loop resumes its single fiber once and returns.

That seed fiber has an 8 MiB stack, the size of a process stack, and larger than a
*spawned* fiber's; `^` recurses as deeply as on a process stack. The seed stack size is fixed
independently of `ulimit -s`: the collector scans a parked fiber's stack whole, and the
seed's size bounds that scan.

## Runnable today

`core.time` — **`@sleep(seconds)`** takes a fractional `Num` and is effect-only (`-> $`): it
waits on the current fiber, then execution continues in program order. It carries no value,
so nothing defers or forces. **`time.now()`** reads a **monotonic** clock in seconds;
*differences* between readings are the meaningful quantity. It is a plain (non-`@`)
primitive — reading the clock is instant. (See `examples/sleep.qn`.)

```quilon
<< core.time

^ = () -> Num => <
  start = time.now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  time.now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep waited → 42
>
```

`core.io` — **`@readStdin() -> Text`** reads one line from stdin. It is value-returning and
therefore deferred: it launches the read, returns immediately, and is forced where a
strict operation reads its bytes. At end-of-input it yields `""`. (See
`examples/readStdin.qn`.)

```quilon
<< core.io

^ = () -> Num => <
  line = @readStdin()          ~ launches the read; returns a deferred Text (no wait here)
  assert(line, equals(""))     ~ the comparison FORCES it; "" at end-of-input (no piped input)
  0
>
~ pipe a line to see a real value flow:  echo hello | quilon run examples/readStdin.qn
```

Binding `line` returns at once; the force is the `==` behind `equals`. `print`/`eprint`
force and write eagerly, and per-fiber output stays in program order.

`core.net` — **`@tcpRequest(address :: Text, requestBytes :: Text) -> Result`** is a one-shot
request exchange: connect to `address` (`host:port`), write the request bytes, read the
response until the peer closes (close-delimited), and hand back a deferred `Result` —
`Ok(responseBytes)` on success or `NotOk(errorMessage)` on any network failure — forced on use
like `@readStdin`. A failure is a value to match; the response is capped at 16 MiB.
The HTTP client sits on it — framing and parsing happen in ordinary Quilon on the forced bytes.

## Where it is headed

A networked value-returning primitive makes independent launches overlap:

```quilon ignore
~ `@get` is a leaf IO primitive (corelib/runtime) — the ONLY marked thing here.
~ `fetchJson` is ordinary, unmarked user code, and concurrency-capable:
fetchJson = (url :: Text) -> Text => < @get(url) > ~ launches IO, returns a deferred Text

loadDashboard = (user :: Text) -> Text => <
  profile = fetchJson("/users/" + user)     ~ launches the first fetch, returns immediately
  orders  = fetchJson("/orders/" + user)    ~ launches the second fetch — overlaps the first
  render(profile, orders)                    ~ each forced at a strict op inside render (block joins)
>
```
