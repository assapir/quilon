~ core.net — networking primitives. Import with `<< core.net`. The HTTP client sits on this,
~ and user code may use the raw request-exchange primitive directly.

~ Perform a one-shot TCP request exchange: connect to `address` (`host:port`), write
~ `requestBytes`, then read the response until the peer closes the connection
~ (close-delimited).
~
~ Returns a `Result`: `Ok(responseBytes)` with the whole response as a Text on success, or
~ `NotOk(errorMessage)` on ANY network failure (DNS resolution, connect, write, or read) — the
~ message names the failing stage and the address. Network failure never crashes the program;
~ match the `Result` to handle it. The response is capped at 16 MiB; a larger one yields `NotOk`.
~ Note: hostname resolution is a BLOCKING DNS lookup on the fiber thread, so a slow lookup stalls
~ the scheduler for now (a numeric `host:port` skips it); non-blocking DNS is a later refinement.
~
~ `@tcpRequest` is a leaf IO primitive (the `@` marker): calling it launches the exchange in
~ the background and hands back a DEFERRED Result immediately — the fiber only waits (forces)
~ once a strict operation reads it (a match, `print`, a native call, ...). The body below is an
~ inert placeholder — the call is compiler-lowered — but it pins BOTH payloads to `Text`, so a
~ caller's `Ok(bytes)` / `NotOk(error)` binding sees the concrete `Text`, not a generic payload.
>> @tcpRequest = (address :: Text, requestBytes :: Text) -> Result =>
  address == "" ? Ok("") : NotOk("")
