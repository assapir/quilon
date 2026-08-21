~ internal.net — INTERNAL socket primitive. Not part of the public surface: the HTTP client
~ sits on this; user code does not import raw sockets. Import with `<< internal.net`.

~ Perform a one-shot TCP request exchange: connect to `address` (`host:port`), write
~ `requestBytes`, then read the response until the peer closes the connection
~ (close-delimited), returning ALL the response bytes as a Text.
~
~ `@tcpRequest` is a leaf IO primitive (the `@` marker): calling it launches the exchange in
~ the background and hands back a DEFERRED Text immediately — the fiber only waits (forces)
~ once a strict operation reads the bytes (a comparison, `print`, a native call, ...). The
~ body below is an inert placeholder; the code generator lowers `@tcpRequest(...)` to the
~ runtime launch/force intrinsics.
>> @tcpRequest = (address :: Text, requestBytes :: Text) -> Text => ""
