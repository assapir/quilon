---
title: "core.net — Networking"
sidebar:
  label: "core.net"
  order: 5
---

# `core.net` — Networking

Import with `<< core.net`. See the [corelib index](README.md).

`@tcpRequest`, the raw TCP request-exchange primitive the HTTP client sits on.

| Function | Effect |
|----------|--------|
| `@tcpRequest(address :: Text, requestBytes :: Text) -> Result` | One-shot request exchange: connect to `address` (`host:port`), write `requestBytes`, read the response until the peer closes (close-delimited). Yields `Ok(responseBytes)` with the whole response as a `Text` on success, or `NotOk(errorMessage)` on ANY network failure (DNS resolution, connect, write, or read) — a failure is a value to match. A value-returning [leaf IO primitive](../concurrency/README.md): the call launches the exchange and hands back a **deferred** `Result`, forced when a strict operation first reads it. |

The response is capped at **16 MiB**; a larger one yields `NotOk`.
Hostname resolution runs the DNS lookup on a resolver thread pool and parks only the calling
fiber until it answers — every other fiber, timer, and socket on the scheduler keeps making
progress while it is in flight. A numeric `host:port` resolves inline with no thread at all.
The pool's base is the process's CPU count (honouring cgroup quotas and CPU affinity), with a
minimum of 4; it starts on the first hostname lookup and its base threads stay for the rest of
the process, reused across lookups. Under more concurrent lookups than base threads, the pool
grows by one thread per lookup that finds every existing thread already busy, up to 16 threads
in total; a lookup beyond that waits in the queue instead. A thread beyond the base exits as
soon as it finds no more lookups waiting. Neither the base nor the ceiling is configurable.

```quilon
<< core.net
<< core.io

reportFailure = (message :: Text) -> Num => <
  io.eprint(message)   ~ error to stderr…
  1                 ~ …and a non-zero exit
>

^ = () -> Num => <
  @tcpRequest("localhost:8080", "GET / HTTP/1.0\r\n\r\n") ?
    | Ok(response) => response.size > 0 ? 0 : 1   ~ forced by the match
    | NotOk(error) => reportFailure(error)
>
```

Being deferred, independent requests on one fiber overlap automatically — each forces where
its outcome is first read. See the
[Concurrency model](../concurrency/README.md), and
`examples/net_request.qn` for a real HTTP GET over `core.net`.
