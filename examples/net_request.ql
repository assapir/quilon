~ A real HTTP GET over `core.net`: builds a minimal HTTP/1.1 request (CRLF-delimited,
~ `Connection: close` so the close-delimited exchange terminates) and sends it with
~ `@tcpRequest`. The call hands back a DEFERRED Text immediately; `.contains` below is the
~ first strict use, which FORCES the exchange and waits for the response bytes. We assert
~ only that the response carries the HTTP status line — the body of example.com changes.
~ NOTE: this makes a LIVE network call to example.com:80 when run.

<< core.net
<< core.test

^ = () -> $ => <
  request = "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
  response = @tcpRequest("example.com:80", request)
  assert(response.contains("HTTP/1.1"))   ~ forces the deferred Text
>
