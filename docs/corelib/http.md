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
  page = Request { method = Get, url = "http://example.com/" }.send() ?
    | Ok(response) => response
    | NotOk(_)     => Response { raw = "" }
  assert(page.status(), equals(200))
  assert(page.body(), contains("Example Domain"))
  assert(page.header("Content-Type"), isOk())
>
```

The module exports **four type names and no free functions**: `Body`, `Method`, `Request`,
`Response`. A request is built and sent through `Request`, a reply read through `Response`.

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
| `send() -> Result` | Perform the request over `core.net` and validate the reply: `Ok(Response)`, or the `NotOk(Text)` the transport reported. A network failure is a value, never a crash. |

Requests go out as **HTTP/1.0** deliberately, until chunked decoding lands: 1.0 forbids the
chunked transfer-encoding this module has no decoder for, so the connection close alone delimits
the body. `Content-Length` counts **bytes** (`.size`), not characters, and a body-bearing method
sends it even for empty content — that is a body of length zero, not the absence of a body, and a
`Content-Length`-less POST draws a 411.

## `Response`

Wrapped and checked in one step: `Response { raw = text }.validate()`.

| Method | Result |
|--------|--------|
| `validate() -> Result` | `Ok(Response)` for a reply worth reading, or `NotOk(Text)` when `raw` does not open with `HTTP` followed by a terminated first line. |
| `status() -> Num` | The status code (`HTTP/1.0 200 OK` → `200`), or `0` when the status line carries no code that is digits throughout — a garbled reply reads as `0` rather than a plausible wrong number. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `header(name :: Text) -> Result` | A header value by name, **case-insensitive**: `Ok(Text)` / `NotOk(Text)`. The value is trimmed, so `X-Empty:` yields `Ok("")`; when a name repeats, the first line wins. |
| `headers() -> []Text` | The header lines, trimmed and without the status line. A line carrying no colon is not a header and is left out. |
| `body() -> Text` | Everything after the blank line, character for character; `""` when the reply has no blank line. |

Replies are read **leniently**: HTTP/1.0 or 1.1, CRLF or bare LF. All four spellings of a blank
line are measured and the **earliest** wins, so a body carrying a blank line in the other
convention cannot move the split. `body()` never consults `Content-Length` — the close delimits
the body — so a wrong or absent `Content-Length` cannot truncate it, and a body carrying its own
CRLF survives intact.

Known gaps: an IPv6 literal host (`http://[::1]/p`) is read as already carrying a port, so the
default `:80` is not appended; and a scheme-less URL whose query itself contains `://` is cut at
that inner occurrence.

The tables above are the whole supported surface; the records' other methods are implementation
detail — Quilon has no per-member visibility yet.

See `examples/http_parse.qn` for reading a reply offline, and `examples/http_get.qn` for a live
GET.

The parser's and serialiser's edge cases are covered by the suite that lives in
`corelib/http.qn` itself, beside the code it tests: the public surface first, the internals it
rests on second. Only the root program's `describe` blocks survive the import resolver, so
`<< core.http` brings you the client and nothing of its tests. The suite runs when the module is
the file being tested:

```bash
quilon test corelib/http.qn
```
