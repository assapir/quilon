---
title: "core.http — HTTP client"
sidebar:
  label: "core.http"
  order: 6
---
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

`Request` and `Response` are **rich but lazy**: a `Request` holds its method and URL and a
`Response` its raw reply text, and each derives a field only when a method asks for it.

## Types

| Type | Shape |
|------|-------|
| `Body` | `{ content :: Text, contentType :: Text }` — the text to send and the media type to advertise. |
| `Method` | `Get / Post(Body) / Put(Body) / Query(Body) / Delete / Head` — the body-bearing methods carry a `Body`. `Query` is RFC 10008's safe, idempotent method with a body. |
| `Request` | `{ method :: Method, url :: Text }` |
| `Response` | `{ raw :: Text }` |

## Free functions

| Function | Result |
|----------|--------|
| `get(url :: Text) -> Result` | Build a GET `Request` for `url` and send it: `Ok(Response)` / `NotOk(Text)`. |
| `parseResponse(raw :: Text) -> Result` | Wrap raw response text as a `Response`: `Ok(Response)`, or `NotOk(Text)` when `raw` does not open with `HTTP` followed by a terminated first line. |

## `Method`

| Method | Result |
|--------|--------|
| `token() -> Text` | The token this method writes in a request line: `GET`, `POST`, `PUT`, `QUERY`, `DELETE`, `HEAD`. |
| `payload() -> Body` | The `Body` a body-bearing method carries; an empty one for the rest. |
| `carriesBody() -> Bool` | Whether this method defines a meaning for an enclosed body — true for `Post` / `Put` / `Query`. |

## `Request`

| Method | Result |
|--------|--------|
| `send() -> Result` | Perform the request over `core.net` and parse the reply: `Ok(Response)`, or the `NotOk(Text)` the transport reported. A network failure is a value, never a crash. |
| `wire() -> Text` | The request serialised to the text that goes on the wire. |
| `authority() -> Text` | The `host[:port]` to connect to — what the `Host` header carries. Any `userinfo@` prefix is dropped. |
| `path() -> Text` | The path the request line asks for; `/` when the URL carries none. A bare query string gets the `/` root it implies (`example.com?a=b` → `/?a=b`), and a `#fragment` is dropped — a fragment never goes on the wire. |
| `authorityEnd() -> Num` | Where the authority ends in `withoutScheme()`: the first `/`, `?` or `#`. |
| `withoutScheme() -> Text` | The URL with any `scheme://` prefix removed. |

Requests go out as **HTTP/1.0**, which forbids the chunked transfer-encoding this module has no
decoder for; the connection close alone delimits the body. `Content-Length` counts **bytes**
(`.size`), not characters, and a body-bearing method sends it even for empty content — that is a
body of length zero, not the absence of a body, and a `Content-Length`-less POST draws a 411.

## `Response`

| Method | Result |
|--------|--------|
| `status() -> Num` | The status code (`HTTP/1.0 200 OK` → `200`), or `0` when the status line carries no code that is digits throughout — a garbled reply reads as `0` rather than a plausible wrong number. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `header(name :: Text) -> Result` | A header value by name, **case-insensitive**: `Ok(Text)` / `NotOk(Text)`. The value is trimmed, so `X-Empty:` yields `Ok("")`; when a name repeats, the first line wins. |
| `headers() -> []Text` | The header lines, trimmed and without the status line. A line carrying no colon is not a header and is left out. |
| `body() -> Text` | Everything after the blank line, character for character; `""` when the reply has no blank line. |
| `head() -> Text` | The status line and the header lines — everything before the blank line — with CRLF endings rewritten to LF. |
| `blankLine() -> Num` | Where the blank line separating head from body begins, or `raw.length` when the reply has none. |

Replies are read **leniently**: HTTP/1.0 or 1.1, CRLF or bare LF. All four spellings of a blank
line are measured and the **earliest** wins, so a body carrying a blank line in the other
convention cannot move the split. `body()` never consults `Content-Length` — the close delimits
the body — so a wrong or absent `Content-Length` cannot truncate it, and a body carrying its own
CRLF survives intact.

Known gaps: an IPv6 literal host (`http://[::1]/p`) is read as already carrying a port, so the
default `:80` is not appended; and a scheme-less URL whose query itself contains `://` is cut at
that inner occurrence.

See `examples/http_parse.qn` for serialising and parsing offline, and `examples/http_get.qn` for
a live GET.

The parser's and serialiser's edge cases are covered by the suite that lives in
`corelib/http.qn` itself, beside the code it tests. Only the root program's `describe` blocks
survive the import resolver, so `<< core.http` brings you the client and nothing of its
tests. The suite runs when the module is the file being tested:

```bash
quilon test corelib/http.qn
```
