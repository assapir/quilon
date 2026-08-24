~ core.net — networking primitives. Import with `<< core.net`. The HTTP client sits on this,
~ and user code may use the raw request-exchange primitive directly.

~ Perform a one-shot TCP request exchange: connect to `address` (`host:port`), write
~ `requestBytes`, then read the response until the peer closes the connection
~ (close-delimited), returning ALL the response bytes as a Text.
~
~ `@tcpRequest` is a leaf IO primitive (the `@` marker): calling it launches the exchange in
~ the background and hands back a DEFERRED Text immediately — the fiber only waits (forces)
~ once a strict operation reads the bytes (a comparison, `print`, a native call, ...). The
~ body below is an inert placeholder — the call is compiler-lowered.
>> @tcpRequest = (address :: Text, requestBytes :: Text) -> Text => ""
