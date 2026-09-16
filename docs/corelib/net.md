---
title: "core.net — Networking"
sidebar:
  label: "core.net"
  order: 5
---

# `core.net` — Networking

Import with `<< core.net`. See the [corelib index](README.md).

`net.@tcpRequest`, the raw TCP request-exchange primitive the HTTP client sits on.

| Function | Effect |
|----------|--------|
| `net.@tcpRequest(address :: Text, requestBytes :: Text) -> Result` | One-shot request exchange: connect to `address` (`host:port`), write `requestBytes`, read the response until the peer closes (close-delimited). Yields `Ok(responseBytes)` with the whole response as a `Text` on success, or `NotOk(errorMessage)` on ANY network failure (DNS resolution, connect, write, or read) — a failure is a value to match. A value-returning [leaf IO primitive](../concurrency/README.md): the call launches the exchange and hands back a **deferred** `Result`, forced when a strict operation first reads it. |

The response is capped at **16 MiB**; a larger one yields `NotOk`.
Hostname resolution runs on the runtime's blocking-call pool and parks only the calling fiber
until it answers, so it never stalls other fibers, timers, or sockets on the scheduler; a
numeric `host:port` resolves inline with no thread at all. See
[Concurrency runtime: blocking calls](../concurrency/runtime.md#blocking-calls) for the pool's
sizing.

```quilon
<< core.net
<< core.io

reportFailure = (message :: Text) -> Num => <
  io.eprint(message)   ~ error to stderr…
  1                 ~ …and a non-zero exit
>

^ = () -> Num => <
  net.@tcpRequest("localhost:8080", "GET / HTTP/1.0\r\n\r\n") ?
    | Ok(response) => response.size > 0 ? 0 : 1   ~ forced by the match
    | NotOk(error) => reportFailure(error)
>
```

Being deferred, independent requests on one fiber overlap automatically — each forces where
its outcome is first read. See the
[Concurrency model](../concurrency/README.md), and
`examples/net_request.qn` for a real HTTP GET over `core.net`.

## The raw TCP server layer

`net.@tcpServe`, the raw TCP server layer: the runtime owns only accepting connections and
running each on its own fiber, and everything protocol-shaped past that is ordinary
Quilon, reached through the connection the accept loop hands the handler.

| Function | Effect |
|----------|--------|
| `net.@tcpServe(address :: Text, handler :: (Connection) -> $) -> Server` | Bind `address` — `host:port`, exactly the form `net.@tcpRequest` accepts (a numeric IPv4/IPv6 address, an IPv6 literal in brackets, or a hostname resolved the same way, on the runtime's blocking-call pool) — listen, and return the `Server` handle at once — never deferred. The accept loop is a launch of the enclosing `< >` block, joined by that block's own [block-scope join](../concurrency/README.md#implemented-primitives), so `^` stays alive while the server runs; a program that never kills its server runs until the process does. Each accepted connection runs `handler` on its own fiber. A bind failure — the address does not parse or resolve, the port is taken, or nothing but a privileged process may bind it — is fatal, naming the address as written. |

`net.Connection`, the value `handler` is called with, one per accepted peer:

| Member | Effect |
|--------|--------|
| `connection.@read() -> Text` | The bytes that have arrived since the connection's last read, as a deferred `Text` — `""` once the peer has closed. Parks on readiness and forces at the first strict use, exactly like `net.@tcpRequest`. |
| `connection.@write(bytes :: Text) -> $` | Write every byte of `bytes`, parking on writability until all of it is sent. Effect-only. |
| `connection.close() -> $` | Close the connection now. A handler that returns without calling this has its connection closed by the runtime. |

`net.Server`, the handle `net.@tcpServe` returns:

| Member | Effect |
|--------|--------|
| `server.kill(seconds :: Num) -> $` | Stop accepting, wait up to `seconds` for in-flight handlers to finish, then close any connection still open and the listener itself. Parks the calling fiber until every one of that has happened. |
| `server.kill() -> $` | `kill` with the default 5-second grace period. |

```quilon
<< core.net
<< core.test

holler = (connection :: net.Connection) -> $ => <
  shout = connection.@read()
  connection.@write(shout)
  $
>

^ = () -> $ => <
  canyon = net.@tcpServe("127.0.0.1:9047", connection => holler(connection))
  net.@tcpRequest("127.0.0.1:9047", "hellooo") ?
    | Ok(echo) => assert(echo, equals("hellooo"))
    | NotOk(error) => test.failAt(error)
  canyon.kill(1)
>
```

Each accepted connection runs on its own fiber, so two peers exchanging bytes with the
server at once make progress independently — see the
[Concurrency model](../concurrency/README.md). `examples/tcp_echo.qn` is the runnable
version of the program above.
