~ A real HTTP GET over `core.net`: builds a minimal HTTP/1.1 request (CRLF-delimited,
~ `Connection: close` so the close-delimited exchange terminates) and sends it with
~ `@tcpRequest`. The call hands back a DEFERRED Result immediately; the `?` match below is the
~ first strict use, which FORCES the exchange and waits for the outcome. On `Ok` we assert the
~ response carries the HTTP status line (the body of example.com changes); on `NotOk` we fail
~ with the network error the primitive reported — a failure is a value here, never a crash.
~ NOTE: this makes a LIVE network call to example.com:80 when run.

<< core.net
<< core.test

^ = () -> $ => <
  request = "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
  @tcpRequest("example.com:80", request) ?
    | Ok(response) => assert(response.contains("HTTP/1.1"))   ~ forced by the match
    | NotOk(error) => failAt(error)
>
