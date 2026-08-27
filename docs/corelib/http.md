# `core.http` — HTTP client

Import with `<< core.http`. See the [corelib index](README.md).

An HTTP client written in Quilon over [`core.net`](net.md)'s `@tcpRequest`. **HTTP only, no
TLS** — URLs are `http://host[:port]/path` (scheme optional, default port 80). Each request
opens one connection, sends `Connection: close`, and reads the close-delimited reply.

```quilon
<< core.http

^ = () -> $ => <
  page = get("http://example.com/") ?
    | Ok(response) => response
    | NotOk(_)     => Response { raw = "" }
  assert(page.status(), equals(200))
  assert(page.body(), contains("Example Domain"))
  assert(page.header("Content-Type"), isOk())
>
```

`Request` and `Response` are **rich but lazy**: each holds the raw text and parses a field only
when a method asks for it.

## Types

| Type | Shape |
|------|-------|
| `Body` | `{ content :: Text, contentType :: Text }` — the text to send and the media type to advertise. |
| `Method` | `Get / Post(Body) / Put(Body) / Query(Body) / Delete / Head` — the body-bearing methods carry a `Body`. |
| `Request` | `{ method :: Method, url :: Text }` |
| `Response` | `{ raw :: Text }` |

## Free functions

Both build a value, so neither belongs on a type.

| Function | Result |
|----------|--------|
| `get(url :: Text) -> Result` | Build a GET `Request` for `url` and send it: `Ok(Response)` / `NotOk(Text)`. |
| `parseResponse(raw :: Text) -> Result` | Wrap raw response text as a `Response`: `Ok(Response)`, or `NotOk(Text)` when `raw` carries no HTTP status line. |

## `Method`

| Method | Result |
|--------|--------|
| `token() -> Text` | The token this method writes in a request line: `GET`, `POST`, `PUT`, `QUERY`, `DELETE`, `HEAD`. |
| `payload() -> Body` | The `Body` a body-bearing method carries; an empty one for the rest. |

`Query` serialises like `Post` — method token `QUERY`, the body, and its `Content-Length`.

## `Request`

| Method | Result |
|--------|--------|
| `send() -> Result` | Perform the request over `core.net` and parse the reply: `Ok(Response)`, or the `NotOk(Text)` the transport reported. A network failure is a value, never a crash. |
| `wire() -> Text` | The request serialised to the text that goes on the wire. |
| `authority() -> Text` | The `host[:port]` to connect to — what the `Host` header carries. |
| `path() -> Text` | The path the request line asks for; `/` when the URL carries none. |
| `withoutScheme() -> Text` | The URL with any `scheme://` prefix removed. |

```quilon
<< core.http

^ = () -> $ => <
  request = Request {
    method = Post(Body { content = "name=ada", contentType = "text/plain" }),
    url = "http://example.com/submit"
  }
  assert(request.wire(), equals(
    "POST /submit HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n"
      + "Content-Type: text/plain\r\nContent-Length: 8\r\n"
      + "\r\nname=ada"))
>
```

Requests go out as **HTTP/1.0**, which forbids the chunked transfer-encoding this module has no
decoder for; the connection close alone delimits the body. `Content-Length` counts **bytes**
(`.size`), not characters.

## `Response`

| Method | Result |
|--------|--------|
| `status() -> Num` | The status code (`HTTP/1.0 200 OK` → `200`), or `0` when the status line carries no code that is digits throughout — a garbled reply reads as `0` rather than a plausible wrong number. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `header(name :: Text) -> Result` | A header value by name, **case-insensitive**: `Ok(Text)` / `NotOk(Text)`. The value is trimmed, so `X-Empty:` yields `Ok("")`; when a name repeats, the first line wins. |
| `headers() -> []Text` | The header lines, trimmed and without the status line. A line carrying no colon is not a header and is left out. |
| `body() -> Text` | Everything after the blank line, character for character; `""` when the reply has no blank line. |
| `head() -> Text` | The status line and the header lines, with CRLF endings normalised to LF. |
| `separator() -> Text` | The blank line this reply separates head from body with: `"\r\n\r\n"`, `"\n\n"`, or `""`. The **earlier** of the two wins, so a body carrying a blank line in the other convention cannot move the split. |

Replies are read **leniently**: HTTP/1.0 or 1.1, CRLF or bare LF. `body()` never consults
`Content-Length` — the close delimits the body — so a wrong or absent `Content-Length` cannot
truncate it, and a body carrying its own CRLF survives intact.

## Examples and tests

| File | What it covers |
|------|----------------|
| `examples/http_test.qn` | The suite: parser and serialiser edge cases, offline over canned `Text`. Run it with `quilon test examples/http_test.qn`. |
| `examples/http_parse.qn` | Serialising and parsing, offline — no network. |
| `examples/http_get.qn` | A **live** GET to example.com. |
