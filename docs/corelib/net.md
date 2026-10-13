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
| `net.@tcpRequest(address :: Text, requestBytes :: Text, options :: ConnectOptions) -> Result` | As the two-argument form, with `options.transport = TLS` running the exchange over TLS first (`options.certificates` deciding whether the peer's certificate is checked) before writing `requestBytes`. `options.transport = Plain` is identical to the two-argument form. |
| `net.@tcpRequest(address :: Address, requestBytes :: Text, options :: ConnectOptions) -> Result` | As above, given an `Address`. |

The response is capped at **16 MiB**; a larger one yields `NotOk`.
Hostname resolution runs on the runtime's blocking-call pool and parks only the calling fiber
until it answers, so it never stalls other fibers, timers, or sockets on the scheduler; a
numeric `host:port` resolves inline with no thread at all. See
[Concurrency runtime: blocking calls](../concurrency/runtime.md#blocking-calls) for the pool's
sizing.

## TLS

`ConnectOptions`, the `TLS`-overload settings:

| Member | Effect |
|--------|--------|
| `options.transport :: Transport` | `Plain` or `TLS`. |
| `options.certificates :: Certificates` | `Checked` or `Unchecked`. |
| `ConnectOptions.default() -> ConnectOptions` | `{ transport = Plain, certificates = Checked }`. |

`Transport = Plain / TLS`. Over `TLS`, the handshake's server name (for SNI, and for the
certificate's name check) is `address`'s host: an IP-literal host uses rustls's IP form of
the server name; a hostname uses its DNS form.

`Certificates = Checked / Unchecked`. `Checked` (the default) verifies the peer's
certificate against the OS trust store — or, when `SSL_CERT_FILE`/`SSL_CERT_DIR` is set,
whichever one [`rustls-native-certs`](https://docs.rs/rustls-native-certs) reads then — and
that the certificate is valid for the address's host. `Unchecked` accepts any certificate,
trusted or not, still requiring the peer to hold the certificate's private key.

The handshake's crypto — key exchange, certificate and signature verification — runs on
the runtime's blocking-call pool, parking only the calling fiber; the network I/O around
it stays on the reactor, and record encryption/decryption after the handshake run inline.
See [Concurrency runtime: blocking calls](../concurrency/runtime.md#blocking-calls).

A handshake or certificate failure yields `NotOk`, naming the address and a reason —
plain English for the common cases, rustls's own text otherwise:

```
NotOk("TLS handshake with api.example.com:443 failed: the certificate is not trusted (issued by an unknown authority)")
NotOk("TLS handshake with api.example.com:443 failed: the certificate expired on 2026-01-04")
NotOk("TLS handshake with api.example.com:443 failed: the certificate is for *.example.org, not api.example.com")
```

```quilon
<< core.net
<< core.test

^ = () -> $ => <
  net.@tcpRequest(
    "example.com:443", "GET / HTTP/1.0\r\n\r\n",
    net.ConnectOptions { transport = net.TLS, certificates = net.Checked }
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
| `net.@tcpServe(address :: Text, handler :: (Connection) -> $) -> Server` | Bind `address` — `host:port`, exactly the form `net.@tcpRequest` accepts (a numeric IPv4/IPv6 address, an IPv6 literal in brackets, or a hostname resolved the same way, on the runtime's blocking-call pool) — listen, and return the `Server` handle at once — never deferred. The accept loop is a launch of the enclosing `< >` block, joined by that block's own [block-scope join](../concurrency/README.md#implemented-primitives), so `^` stays alive while the server runs; a program that never kills its server runs until the process does. Each accepted connection runs `handler` on its own fiber. A bind failure — the address does not parse or resolve, the port is taken, or nothing but a privileged process may bind it — is fatal, naming the address as written. This form always means one `Plain` listener. |
| `net.@tcpServe(address :: Address, handler :: (Connection) -> $) -> Server` | As above, given the `Address` a `server.address()` reported. |
| `net.@tcpServe(listeners :: []Listener, handler :: (Connection) -> $) -> Server` | Bind every `Listener` in `listeners` — `Plain` or `TLS` per its own `transport`/`identity` — and return ONE `Server` handle: `kill` stops every listener, `addresses()` reports all of their bound addresses. See [TLS listeners](#tls-listeners). |

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
| `server.address() -> Address` | The address this server bound. Binding port `0` takes any free port the OS assigns, and this is how a program learns which one. Fatal on a server with more than one listener (see [TLS listeners](#tls-listeners)) — call `addresses()` instead. |
| `server.addresses() -> []Address` | Every listener's bound address, in the order `net.@tcpServe`/`http.@serve` was given them — a one-element list for a server with a single listener. |

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

## TLS listeners

`Listener`, one socket a server binds, and `Identity`, its TLS identity:

| Type | Effect |
|------|--------|
| `Listener { address :: Text, transport :: Transport, identity :: Identity }` | `address` is `host:port`, exactly the form `net.@tcpServe` otherwise accepts. |
| `Identity = None / Files(certificatePath :: Text, keyPath :: Text) / Base64(certificate :: Text, key :: Text)` | `None` for a `Plain` listener. `Files` reads two PEM files when the listener starts; the certificate file may hold a whole chain, leaf first. `Base64` is the base64 of each of those same two PEM files, one line each (`base64 -w0 fullchain.pem`), for an environment variable or a secrets manager — it decodes to PEM and goes through the same parser. Either way, the private key may be PKCS#8, SEC1, or PKCS#1. |

`transport = TLS` with `identity = None`, or `transport = Plain` with an identity other
than `None`, is a compile error ([QN359](../tooling/errors/semantics.md#qn359--mismatched-listener-transport-and-identity))
on a `Listener` written as a literal; on one assembled another way (from a variable, a
function's return value), it is fatal when the listener starts
([QN512](../tooling/errors/runtime.md#qn512--mismatched-listener-transport-and-identity-at-startup)).
An identity that cannot be loaded — a missing file, base64 that does not decode, bytes
that are not a valid certificate chain or private key, or a key that does not match the
certificate — is fatal the same way, naming the listener's address and a reason
([QN513](../tooling/errors/runtime.md#qn513--tls-identity-failed-to-load)).

`net.@tcpServe`'s listener-list form builds each `TLS` listener's `ServerConfig` ONCE,
when the listener starts, from its `identity` — never rebuilt per connection. Each
accepted connection then runs rustls's server-side handshake — the same split as the
client's own TLS (crypto on the runtime's blocking-call pool, record I/O on the reactor,
encryption/decryption inline afterward) — before `handler` ever sees it; a client whose
handshake fails is closed silently, with `handler` never called. Past the handshake, a
`TLS` connection is handed to `handler` as the very same `Connection` every `Plain` one
is: `@read`/`@write`/`close` work unchanged.

```quilon
<< core.net
<< core.test

respond = (connection :: net.Connection) -> $ => <
  connection.@write(connection.@read())
  $
>

^ = () -> $ => <
  counter = net.@tcpServe(
    [
      net.Listener { address = "127.0.0.1:0", transport = net.Plain, identity = net.None },
      net.Listener {
        address = "127.0.0.1:0", transport = net.TLS,
        identity = net.Files("cert.pem", "key.pem")
      }
    ],
    connection => respond(connection))
  assert(counter.addresses().size, equals(2))
  counter.kill(1)
>
```
