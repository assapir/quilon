---
title: "core.http — HTTP client"
sidebar:
  label: "core.http"
  order: 6
---
# `core.http` — HTTP client

Import with `<< core.http`. See the [corelib index](README.md).

An HTTP client written in Quilon over [`core.net`](net.md)'s `@tcpRequest`. The scheme is
**plain HTTP** — URLs are `http://host[:port]/path` (scheme optional, default port 80). Each
request opens one connection, sends `Connection: close`, and reads the close-delimited reply.

```quilon
<< core.http

^ = () -> $ => <
  page = http.Request { method = http.Get, url = "http://example.com/" }.send() ?
    | Ok(response) => response
    | NotOk(_)     => http.Response { raw = "" }
  assert(page.status(), equals(200))
  assert(page.body(), contains("Example Domain"))
  assert(page.header("Content-Type"), isOk())
>
```

The module exports **four type names**: `Body`, `Method`, `Request`, `Response`. A request
is built and sent through `Request`, a reply read through `Response`.

`Request` and `Response` are **rich but lazy**: a `Request` holds its method and URL and a
`Response` its raw reply text, and each derives a field only when a method asks for it.

## Types

| Type | Shape |
|------|-------|
| `Body` | `{ content :: Text, contentType :: Text }` — the text to send and the media type to advertise. |
| `Method` | `Get / Post(Body) / Put(Body) / Query(Body) / Delete / Head` — the body-bearing methods carry a `Body`. `Query` is RFC 10008's safe, idempotent method with a body. |
| `Request` | `{ method :: Method, url :: Text }` |
| `Response` | `{ raw :: Text }` |

## `Method`

| Method | Result |
|--------|--------|
| `token() -> Text` | The token this method writes in a request line: `GET`, `POST`, `PUT`, `QUERY`, `DELETE`, `HEAD`. |
| `payload() -> Body` | The `Body` a body-bearing method carries; an empty one for the rest. |
| `carriesBody() -> Bool` | Whether this method defines a meaning for an enclosed body — true for `Post` / `Put` / `Query`. |

## `Request`

| Method | Result |
|--------|--------|
| `send() -> Result` | Perform the request over `core.net` and validate the reply: `Ok(Response)`, or the `NotOk(Text)` the transport reported. A network failure is a value to match. |

Requests go out as **HTTP/1.0**: the connection close delimits the body. `Content-Length`
counts **bytes** (`.size`), and a body-bearing method sends it for empty content too — a
body of length zero.

## `Response`

Wrapped and checked in one step: `http.Response { raw = text }.validate()`.

| Method | Result |
|--------|--------|
| `validate() -> Result` | `Ok(Response)` when `raw` opens with `HTTP` followed by a terminated first line, `NotOk(Text)` otherwise. |
| `status() -> Num` | The status code (`HTTP/1.0 200 OK` → `200`); `0` when the status line's code is anything other than digits throughout. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `header(name :: Text) -> Result` | A header value by name, **case-insensitive**: `Ok(Text)` / `NotOk(Text)`. The value is trimmed, so `X-Empty:` yields `Ok("")`; when a name repeats, the first line wins. |
| `headers() -> []Text` | The header lines that carry a colon, trimmed, after the status line. |
| `body() -> Text` | Everything after the blank line, character for character; `""` when the reply has no blank line. |

Replies are read **leniently**: HTTP/1.0 or 1.1, CRLF or bare LF. All four spellings of a blank
line are measured and the **earliest** wins. The close delimits the body: `body()` reads to
the close independently of `Content-Length`, and a body carrying its own CRLF survives intact.

Known limits: an IPv6 literal host (`http://[::1]/p`) is read as already carrying a port, and
the default `:80` is left off; a scheme-less URL whose query itself contains `://` is cut at
that inner occurrence.

The tables above are the supported surface; the records' other methods are implementation
detail.

See `examples/http_parse.qn` for reading a reply offline, and `examples/http_get.qn` for a live
GET.

The parser's and serialiser's edge cases are covered by the suite that lives in
`corelib/http.qn` itself, beside the code it tests: the public surface first, the internals it
rests on second. The root program's `describe` blocks are the ones the import resolver keeps,
and the fixtures under them carry no `>>`, so `<< core.http` brings the client alone. The
harness it runs under is a plain `<< core.test`, which an importer resolves, so `describe`,
`it` and the rest of that module's exports are in scope too. The suite runs when the module
is the file being tested:

```bash
quilon test corelib/http.qn
```
