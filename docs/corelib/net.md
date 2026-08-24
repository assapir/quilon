# `core.net` — Networking

Import with `<< core.net`. See the [Standard library index](../LANGUAGE.md#standard-library).

`core.net` provides `@tcpRequest`, the raw TCP request-exchange primitive the HTTP client sits on. It is compiler-lowered to the runtime launch/force intrinsics.

| Function | Effect |
|----------|--------|
| `@tcpRequest(address :: Text, requestBytes :: Text) -> Text` | Perform a one-shot TCP request exchange: connect to `address` (`host:port`), write `requestBytes`, then read the response until the peer closes the connection (close-delimited), returning all the response bytes as a `Text`. A value-returning [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) (the `@` marker): the call launches the exchange in the background and hands back a **deferred** `Text` immediately, forced on use exactly like `@readStdin` — the fiber only waits once a strict operation reads the bytes (a comparison, `print`, a native call). |

```quilon
<< core.net

^ = () -> Num => <
  response = @tcpRequest("localhost:8080", "GET / HTTP/1.0\r\n\r\n")
  response.size > 0 ? 0 : 1   ~ forced by the comparison
>
```

Because `@tcpRequest` returns a deferred value, independent requests launched on the same
fiber overlap automatically — each forces only where its bytes are first read. See the
[Concurrency model](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress) for
how `@` leaf primitives, deferred values, and force-at-strict-op fit together.

For a runnable end-to-end program that makes a real HTTP GET over `core.net`, see
`examples/net_request.ql`.
