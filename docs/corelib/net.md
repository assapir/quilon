# `core.net` — Networking

Import with `<< core.net`. See the [corelib index](../LANGUAGE.md#corelib).

`@tcpRequest`, the raw TCP request-exchange primitive the HTTP client sits on, compiler-lowered to the runtime launch/force intrinsics.

| Function | Effect |
|----------|--------|
| `@tcpRequest(address :: Text, requestBytes :: Text) -> Text` | One-shot request exchange: connect to `address` (`host:port`), write `requestBytes`, read the response until the peer closes (close-delimited), and return all of it as a `Text`. A value-returning [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress): the call launches the exchange and hands back a **deferred** `Text`, forced when a strict operation first reads the bytes. |

```quilon
<< core.net

^ = () -> Num => <
  response = @tcpRequest("localhost:8080", "GET / HTTP/1.0\r\n\r\n")
  response.size > 0 ? 0 : 1   ~ forced by the comparison
>
```

Being deferred, independent requests on one fiber overlap automatically — each forces where
its bytes are first read. See the
[Concurrency model](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress), and
`examples/net_request.ql` for a real HTTP GET over `core.net`.
