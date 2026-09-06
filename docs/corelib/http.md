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
  page = http.Request.get("http://example.com/").send() ?
    | Ok(response) => response
    | NotOk(_)     => http.Response { raw = "" }
  assert(page.status(), equals(200))
  assert(page.body(), contains("Example Domain"))
  assert(page.headers().get("Content-Type"), isOk())
>
```

The module exports **seven type names**: `Body`, `Method`, `Headers`, `Params`,
`RequestOptions`, `Request`, `Response`. A request is built through `Request`'s static
constructors and sent, a reply read through `Response`.

`Request` and `Response` are **rich but lazy**: a `Request` holds its method, URL and
options and a `Response` its raw reply text, and each derives a field only when a method
asks for it.

## Types

| Type | Shape |
|------|-------|
| `Body` | `{ content :: Text, contentType :: Text }` — the text to send and the media type to advertise. |
| `Method` | `Get / Post(Body) / Put(Body) / Query(Body) / Delete / Head / Options / Patch(Body)` — the body-bearing methods carry a `Body`. `Query` is RFC 10008's safe, idempotent method with a body; `Patch` carries a body the same way `Post` and `Put` do. |
| `Headers` | A multi-value, case-insensitive name/value store, one name added through `add`, read through `get`/`all`/`has`/`names`. |
| `Params` | A multi-value, case-sensitive name/value store, the same surface as `Headers` without the case-folding. |
| `RequestOptions` | `{ headers :: Headers }` — settings a request carries beyond its method and URL. |
| `Request` | `{ method :: Method, url :: Text, options :: RequestOptions }` |
| `Response` | `{ raw :: Text }` |

## `Method`

| Method | Result |
|--------|--------|
| `token() -> Text` | The token this method writes in a request line: `GET`, `POST`, `PUT`, `QUERY`, `DELETE`, `HEAD`, `OPTIONS`, `PATCH`. |
| `payload() -> Body` | The `Body` a body-bearing method carries; an empty one for the rest. |
| `carriesBody() -> Bool` | Whether this method defines a meaning for an enclosed body — true for `Post` / `Put` / `Query` / `Patch`. |

## `Headers` and `Params`

Both keep every value added under a name, in the order added — HTTP treats a repeated
header (`X-A: 1` then `X-A: 2`) as one name carrying two values, and a repeated query
parameter (`tag=a&tag=b`) the same way. `Headers` lower-cases every name it is given, HTTP/2
style, before storing or looking one up; `Params` compares names exactly as written.

| Method | Result |
|--------|--------|
| `empty() -> Headers` / `Params` | A value with nothing stored (static). |
| `get(name :: Text) -> Result` | The first value under `name`: `Ok(Text)`, or `NotOk(name)` when absent. |
| `all(name :: Text) -> []Text` | Every value under `name`, in the order added; `[]` when absent. |
| `has(name :: Text) -> Bool` | Whether any value is stored under `name`. |
| `names() -> []Text` | Every name carrying at least one value. |
| `add(name :: Text, value :: Text)` | A setter: appends `value` under `name`, keeping any already there. |
| `add(entries :: [|Text => Text|])` | A setter: appends every entry of `entries`, one name at a time. |

A caller builds an empty `RequestOptions`, then adds headers to it, before passing it to a
constructor:

```quilon
<< core.http

^ = () -> Num => <
  options := http.RequestOptions.default()
  options.headers.add("X-Trace", "abc")
  options.headers.add([|"Accept" => "application/json"|])
  request := http.Request.get("http://example.com/", options)
  request.options.headers.names().size
>
```

Params have no percent-encoding on the way out and no percent-decoding on the way in.

## `RequestOptions`

| Method | Result |
|--------|--------|
| `default() -> RequestOptions` | Empty headers (static). |

## `Request`

A request is built through a static constructor — one per method, each with a two-argument
overload that takes an explicit `RequestOptions`:

| Constructor | Result |
|-------------|--------|
| `get(url)` / `get(url, options)` | A `Get` request. |
| `head(url)` / `head(url, options)` | A `Head` request. |
| `delete(url)` / `delete(url, options)` | A `Delete` request. |
| `options(url)` / `options(url, options)` | An `Options` request. |
| `post(url, body)` / `post(url, body, options)` | A `Post(body)` request. |
| `put(url, body)` / `put(url, body, options)` | A `Put(body)` request. |
| `patch(url, body)` / `patch(url, body, options)` | A `Patch(body)` request. |
| `query(url, body)` / `query(url, body, options)` | A `Query(body)` request. |

The one-argument form carries `RequestOptions.default()`. A dot-call on the type name
(`Request.options(url)`) reaches the constructor; a bare access on a value (`request.options`)
reaches the `options` field instead.

| Method | Result |
|--------|--------|
| `params() -> Params` | The URL's query string, parsed (`?q=ada&tag=a&tag=b` → `q` carries `["ada"]`, `tag` carries `["a", "b"]`); no params when the URL carries none. |
| `headers() -> Headers` | The request's own headers (`it.options.headers`). |
| `send() -> Result` | Perform the request over `core.net` and validate the reply: `Ok(Response)`, or the `NotOk(Text)` the transport reported. A network failure is a value to match. |

Requests go out as **HTTP/1.0**: the connection close delimits the body. `Content-Length`
counts **bytes** (`.size`), and a body-bearing method sends it for empty content too — a
body of length zero. Every generated header (`host`, `connection`, `content-type`,
`content-length`) goes on the wire lower-cased; a caller header sharing one of those names
replaces the generated line rather than duplicating it.

## `Response`

Wrapped and checked in one step: `http.Response { raw = text }.validate()`.

| Method | Result |
|--------|--------|
| `validate() -> Result` | `Ok(Response)` when `raw` opens with `HTTP` followed by a terminated first line, `NotOk(Text)` otherwise. |
| `status() -> Num` | The status code (`HTTP/1.0 200 OK` → `200`); `0` when the status line's code is anything other than digits throughout. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `headers() -> Headers` | The reply's headers, parsed from the lines between the status line and the blank line. |
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
