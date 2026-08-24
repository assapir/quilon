# `core.net` — Networking

Import with `<< core.net`. See the [corelib index](../LANGUAGE.md#corelib).

`@tcpRequest`, the raw TCP request-exchange primitive the HTTP client sits on.

| Function | Effect |
|----------|--------|
| `@tcpRequest(address :: Text, requestBytes :: Text) -> Result` | One-shot request exchange: connect to `address` (`host:port`), write `requestBytes`, read the response until the peer closes (close-delimited). Yields `Ok(responseBytes)` with the whole response as a `Text` on success, or `NotOk(errorMessage)` on ANY network failure (DNS resolution, connect, write, or read) — a failure is a value to match, never a crash. A value-returning [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress): the call launches the exchange and hands back a **deferred** `Result`, forced when a strict operation first reads it. |

The response is capped at **16 MiB**; a larger one yields `NotOk` rather than exhausting memory.
Hostname resolution is a **blocking** DNS lookup on the fiber thread, so a slow lookup stalls the
scheduler (a numeric `host:port` skips it); non-blocking DNS is a later refinement.

```quilon
<< core.net
<< core.io

^ = () -> Num => <
  @tcpRequest("localhost:8080", "GET / HTTP/1.0\r\n\r\n") ?
    | Ok(response) => response.size > 0 ? 0 : 1   ~ forced by the match
    | NotOk(error) => <
        eprint(error)
        1
      >
>
```

Being deferred, independent requests on one fiber overlap automatically — each forces where
its outcome is first read. See the
[Concurrency model](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress), and
`examples/net_request.ql` for a real HTTP GET over `core.net`.
