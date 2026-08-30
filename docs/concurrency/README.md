---
title: "Concurrency — colorless implicit futures (🚧 in progress)"
sidebar:
  label: "Concurrency"
---

# Concurrency — colorless implicit futures (🚧 in progress)

> Colorless implicit futures on cooperative fibers: IO returns type-invisible deferreds, only strict operations force them — concurrency follows data dependence, not program order.

> **Status: 🚧 in progress.** The model below is locked, and its core runs: the
> single-threaded fiber scheduler, the effect-only `@sleep` pause (`core.time`), the
> deferred-value `@readStdin` (`core.io`), and the networked `@tcpRequest` (`core.net`).
> Not yet: **overlap** as a showcase (two independent reads finishing in max-time rather
> than sum-time), which needs a primitive like `@get`; and the multicore (M:N) runtime.

Quilon's concurrency is **colorless**: you write ordinary, blocking-*looking* code and the
runtime overlaps independent IO for you. No `async`, no `await`, no `go`/`spawn`, no resolve
token, and no **function coloring** — a function that does IO is written and typed exactly
like one that doesn't. `async`/`await` colors every function on the IO path; Go and Loom
still need an explicit `go`. The nearest precedent is **promise pipelining** (E, Cap'n Proto).

**`@` marks leaf IO primitives only** — the corelib/runtime primitives that actually do IO
(`http.get`, a file read, a socket receive, `sleep`). All user code is unmarked: a function that
transitively calls an `@` primitive is concurrency-capable for free, with **no propagation**
up the call chain. That absence of propagation is what makes the model colorless.

**Deferred values.** Calling an `@` primitive launches the IO and returns immediately with a
*deferred* value, without parking the caller. Deferred-ness propagates as the value flows —
passed as an argument, stored in a record or array, returned from a function — forcing
nothing along the way. That lazy threading is the *pipelining*.

**Forcing happens at the leaves.** A deferred value is forced — the fiber parks until it is
ready — only at a **strict** operation: arithmetic, comparison, pattern match (`?`), IO
output (`print`/`write`), and native calls. Values *launched before they are forced* therefore
overlap automatically, with nothing written to ask for it.

**Deferral is type-invisible.** A deferred `Text` types as `Text`, so it does not disturb
exact-type [overload resolution](../functions/overloading.md).

**Structured & scoped.** Deferred tasks are scoped to their enclosing `< >` block: the block
forces and joins everything it launched before returning, and a panic propagates out.

**Why it can be colorless.** Each fiber is **stackful**, so any function
can park at a force point without the compiler rewriting it into a state machine.

**Determinism.** Pure results are fully deterministic. The **ordering of side effects** across
independent deferred IO is unspecified — the accepted cost of implicit overlap.

**A program's entry always runs on the fiber scheduler**, so every `@` primitive it reaches
has a fiber to park on. A pure program pays the scheduler's fixed start-up — a reactor and one
fiber stack — and nothing else. It never parks, so the loop resumes its single fiber once and
returns.

That seed fiber gets an 8 MiB stack, the usual process-stack default and much larger than a
*spawned* fiber's. So `^` recurses about as deeply as it would on an ordinary process stack.
A raised `ulimit -s` does not raise it further: the collector scans a parked fiber's stack
whole, so a bigger seed would cost every collection taken while it is parked.

## Runnable today

`core.time` — **`@sleep(seconds)`** takes a fractional `Num` and is effect-only (`-> $`): it
waits right there on the current fiber, then execution continues in program order. It carries
no value, so nothing defers or forces. **`now()`** reads a **monotonic** clock in seconds;
only *differences* between readings are meaningful. It is a plain (non-`@`) primitive —
reading the clock is instant and never parks. (See `examples/sleep.qn`.)

```quilon
<< core.time

^ = () -> Num => <
  start = now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep really waited → 42
>
```

`core.io` — **`@readStdin() -> Text`** reads one line from stdin. Being value-returning makes
it the deferred one: it launches the read, returns immediately, and is forced only where a
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

Binding `line` does not wait; the force is the `==` behind `equals`. Because
`print`/`eprint` force and write eagerly, per-fiber output stays in program order.

`core.net` — **`@tcpRequest(address :: Text, requestBytes :: Text) -> Result`** is a one-shot
request exchange: connect to `address` (`host:port`), write the request bytes, read the
response until the peer closes (close-delimited), and hand back a deferred `Result` —
`Ok(responseBytes)` on success or `NotOk(errorMessage)` on any network failure — forced on use
like `@readStdin`. A failure is a value to match, never a crash; the response is capped at 16 MiB.
The HTTP client sits on it — framing and parsing happen in ordinary Quilon on the forced bytes.

## Where it is headed

A networked value-returning primitive makes independent launches overlap, which is the reason
implicit futures matter:

```quilon ignore
~ `@get` is a leaf IO primitive (corelib/runtime) — the ONLY marked thing here.
~ `fetchJson` is ordinary, unmarked user code, yet concurrency-capable for free:
fetchJson = (url :: Text) -> Text => @get(url)   ~ launches IO, returns a deferred Text

loadDashboard = (user :: Text) -> Text => <
  profile = fetchJson("/users/" + user)     ~ launches the first fetch, returns immediately
  orders  = fetchJson("/orders/" + user)    ~ launches the second fetch — overlaps the first
  render(profile, orders)                    ~ each forced at a strict op inside render (block joins)
>
```
