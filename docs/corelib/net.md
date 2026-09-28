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
| `net.@tcpRequest(address :: Address, requestBytes :: Text) -> Result` | As above, given the `Address` a `server.address()` reported. |
| `net.@tcpRequest(address :: Text, requestBytes :: Text, options :: ConnectOptions) -> Result` | As the two-argument form, with `options.transport = Tls` running the exchange over TLS first (`options.certificates` deciding whether the peer's certificate is checked) before writing `requestBytes`. `options.transport = Plain` is identical to the two-argument form. |
| `net.@tcpRequest(address :: Address, requestBytes :: Text, options :: ConnectOptions) -> Result` | As above, given an `Address`. |

The response is capped at **16 MiB**; a larger one yields `NotOk`.
Hostname resolution runs on the runtime's blocking-call pool and parks only the calling fiber
until it answers, so it never stalls other fibers, timers, or sockets on the scheduler; a
numeric `host:port` resolves inline with no thread at all. See
[Concurrency runtime: blocking calls](../concurrency/runtime.md#blocking-calls) for the pool's
sizing.

## TLS

`ConnectOptions`, the `Tls`-overload settings:

| Member | Effect |
|--------|--------|
| `options.transport :: Transport` | `Plain` or `Tls`. |
| `options.certificates :: Certificates` | `Checked` or `Unchecked`. |
| `ConnectOptions.default() -> ConnectOptions` | `{ transport = Plain, certificates = Checked }`. |

`Transport = Plain / Tls`. Over `Tls`, the handshake's server name (for SNI, and for the
certificate's name check) is `address`'s host — an IP-literal host uses rustls's IP form of
the server name rather than a DNS name.

`Certificates = Checked / Unchecked`. `Checked` (the default) verifies the peer's
certificate against the OS trust store — or `SSL_CERT_FILE`/`SSL_CERT_DIR` when either is
set, which [`rustls-native-certs`](https://docs.rs/rustls-native-certs) reads instead of the
OS store — and that the certificate is valid for the address's host. `Unchecked` accepts any
certificate, trusted or not, still requiring the peer to hold the certificate's private key.

A handshake or certificate failure yields `NotOk`, naming the address and a reason —
plain English for the common cases, rustls's own text otherwise:

```
NotOk("tls handshake with api.example.com:443 failed: the certificate is not trusted (issued by an unknown authority)")
NotOk("tls handshake with api.example.com:443 failed: the certificate expired on 2026-01-04")
NotOk("tls handshake with api.example.com:443 failed: the certificate is for *.example.org, not api.example.com")
```

```quilon
<< core.net
<< core.test

^ = () -> $ => <
  net.@tcpRequest(
    "example.com:443", "GET / HTTP/1.0\r\n\r\n",
    net.ConnectOptions { transport = net.Tls, certificates = net.Checked }
  ) ?
    | Ok(response)  => assert(response.size > 0, equals(true))
    | NotOk(error)  => test.failAt(error)
>
```

### Trusting a private CA

A certificate authority not in the OS trust store — a private CA for an internal service,
say — is trusted the same two ways any TLS client on the machine trusts it: installed into
the OS trust store, or named by the `SSL_CERT_FILE` (a PEM file) or `SSL_CERT_DIR`
environment variable when the program runs. `core.net` has no API of its own for this —
[`rustls-native-certs`](https://docs.rs/rustls-native-certs) reads both, and a Quilon program
built with `quilon build` needs nothing installed beyond that variable or store entry.

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
| `net.@tcpServe(address :: Address, handler :: (Connection) -> $) -> Server` | As above, given the `Address` a `server.address()` reported. |

`net.Connection`, the value `handler` is called with, one per accepted peer:

| Member | Effect |
|--------|--------|
| `connection.@read() -> Text` | The bytes that have arrived since the connection's last read, as a deferred `Text` — `""` once the peer has closed. Parks on readiness and forces at the first strict use, exactly like `net.@tcpRequest`. |
| `connection.@read(seconds :: Num) -> Text` | As `@read()`, but gives up and yields `""` once `seconds` pass with nothing arriving, exactly as it yields `""` on a peer close — neither carries a channel a `Text` result could tell them apart through. Parks on readiness and the deadline together, racing whichever comes first, the way [`core.time`'s `@sleep`](time.md) parks on a deadline alone. |
| `connection.@write(bytes :: Text) -> $` | Write every byte of `bytes`, parking on writability until all of it is sent. Effect-only. |
| `connection.close() -> $` | Close the connection now. A handler that returns without calling this has its connection closed by the runtime. |

`net.Server`, the handle `net.@tcpServe` returns:

| Member | Effect |
|--------|--------|
| `server.kill(seconds :: Num) -> $` | Stop accepting, wait up to `seconds` for in-flight handlers to finish, then close any connection still open and the listener itself. Parks the calling fiber until every one of that has happened. |
| `server.kill() -> $` | `kill` with the default 5-second grace period. |
| `server.address() -> Address` | The address this server bound. Binding port `0` takes any free port the OS assigns, and this is how a program learns which one. |

`net.Address`, `server.address()`'s own value:

| Member | Effect |
|--------|--------|
| `address.host :: Text` | The bound host as the OS reports it (`127.0.0.1`, `::1`). |
| `address.port :: Num` | The port the OS bound. |
| `address.text() -> Text` | `host:port`, exactly the form `@tcpRequest`/`@tcpServe` accept — an IPv6 `host` wrapped in brackets. |

```quilon
<< core.net
<< core.test

holler = (connection :: net.Connection) -> $ => <
  ~ "" means either the peer closed or 2 seconds passed with nothing sent.
  shout = connection.@read(2)
  connection.@write(shout)
  $
>

^ = () -> $ => <
  canyon = net.@tcpServe("127.0.0.1:0", connection => holler(connection))
  net.@tcpRequest(canyon.address(), "hellooo") ?
    | Ok(echo) => assert(echo, equals("hellooo"))
    | NotOk(error) => test.failAt(error)
  canyon.kill(1)
>
```

Each accepted connection runs on its own fiber, so two peers exchanging bytes with the
server at once make progress independently — see the
[Concurrency model](../concurrency/README.md). `examples/tcp_echo.qn` is the runnable
version of the program above.
