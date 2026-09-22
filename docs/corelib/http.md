---
title: "core.http — HTTP client and server"
sidebar:
  label: "core.http"
  order: 6
---
# `core.http` — HTTP client and server

Import with `<< core.http`. See the [corelib index](README.md).

An HTTP client and server written in Quilon over [`core.net`](net.md)'s `net.@tcpRequest`
and `net.@tcpServe`. The scheme is **plain HTTP** — URLs are `http://host[:port]/path`
(scheme optional, default port 80). The client sends `Connection: close` on every request,
over HTTP/1.1, and opens one connection per request; the server keeps a connection open
across requests by default — see [Keep-alive](#keep-alive).

```quilon
<< core.http

^ = () -> $ => <
  page = http.Request.get("http://example.com/").send() ?
    | Ok(response) => response
    | NotOk(_)     => http.Response { raw = "" }
  assert(page.status().code(), equals(200))
  assert(page.body(), contains("Example Domain"))
  assert(page.headers().get("Content-Type"), isOk())
>
```

The module exports **eight type names**: `Body`, `Method`, `Status`, `Headers`, `Params`,
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
| `Status` | One variant per standard HTTP status code, plus `Other(Num)` — see [`Status`](#status) below. |
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

## `Status`

One variant per code the IANA HTTP status code registry lists as standard (every 1xx–5xx
code the registry assigns), named from its reason phrase in CamelCase — `NotFound` for
`404`, `MethodNotAllowed` for `405` — except the 200 variant, spelled `OK` exactly so it
never reads like `Result`'s `Ok`. `Other(Num)` carries any code outside that table.

| Method | Result |
|--------|--------|
| `code() -> Num` | The numeral this status carries on the wire: `OK` → `200`, `Other(n)` → `n`. |
| `text() -> Text` | Its reason phrase on the wire: `OK` → `"OK"`, `NotFound` → `"Not Found"`, `Other(_)` → `"Unknown"`. |
| `Status.parse(code :: Num) -> Status` | The variant a numeric code names — `404` → `NotFound` — or `Other(code)` for a code the table does not carry. Never fails: every `Num` names some `Status`. |

`code()` and `text()` are each their own match over `it`, the same shape `Method.token()`
uses; `Status.parse(200).code()` and `OK.code()` agree, and so do `.text()`.

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
| `send() -> Result` | Perform the request over `core.net`, validate the reply, and check its body framing: `Ok(Response)`, or the `NotOk(Text)` the transport, the status line, or the framing reported. A network failure, a malformed status line, or a malformed body are each a value to match. |

Requests go out as **HTTP/1.1**: `Connection: close` keeps the close-delimited read valid for
a reply that gives neither `Transfer-Encoding` nor `Content-Length`. `Content-Length` counts
**bytes** (`.size`), and a body-bearing method sends it for empty content too — a body of
length zero. Every generated header (`host`, `connection`, `content-type`, `content-length`)
goes on the wire lower-cased; a caller header sharing one of those names replaces the
generated line.

`send()`'s framing check runs `body()`'s own rule (below) and turns a malformed result into
`NotOk`, with a reason naming what went wrong: `"malformed chunked framing"` for a bad chunk
size, a missing terminator, or data ending early; `"malformed Content-Length"` for a
non-numeric value; `"truncated body: expected N bytes, got M"` for a `Content-Length` the
reply's bytes fall short of. A `Head` request's reply carries a `Content-Length` for a body
that is never sent, so `send()` skips the framing check for it.

## `Response`

Wrapped and checked in one step: `http.Response { raw = text }.validate()`.

| Method | Result |
|--------|--------|
| `validate() -> Result` | `Ok(Response)` when `raw` opens with `HTTP` followed by a terminated first line, `NotOk(Text)` otherwise. |
| `status() -> Status` | The reply's status, parsed off the status line through `Status.parse`; `Other(0)` when the status line's code is anything other than digits throughout. |
| `statusLine() -> Text` | The reply's first line, trimmed. |
| `headers() -> Headers` | The reply's headers, parsed from the lines between the status line and the blank line. |
| `body() -> Text` | The reply's body, framed per its headers (below); `""` when the reply has no blank line, carries no body by its status, or its framing is malformed. |

Replies are read **leniently**: HTTP/1.0 or 1.1, CRLF or bare LF. All four spellings of a blank
line are measured and the **earliest** wins.

`body()` frames the bytes after the blank line by what the head established, in order: an
informational (1xx), a `204`, or a `304` status carries no body, whatever its headers claim.
Otherwise, a `Transfer-Encoding` naming `chunked` (the last of a comma list, case-insensitive)
is dechunked — each chunk a hex size line (a trailing `;extension` ignored), exactly that many
bytes, and a terminating CRLF, ending at a zero-size chunk; any trailers up to the reply's own
end are dropped unparsed. Otherwise, a `Content-Length` takes exactly that many bytes. With
neither header, the close delimits the body, and a body carrying its own CRLF survives intact.
This framing is native — a chunk or a `Content-Length` count cuts bytes, while `Text` slices
by grapheme, so a boundary inside a multi-byte character has no Quilon-reachable position.

Known limits: an IPv6 literal host (`http://[::1]/p`) is read as already carrying a port, and
the default `:80` is left off; a scheme-less URL whose query itself contains `://` is cut at
that inner occurrence.

The tables above are the supported surface; the records' other methods are implementation
detail.

See `examples/http.qn` for a live walkthrough: a GET and a transport failure, request
options and headers, params, every request constructor, and `Method`'s own methods.

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

## The HTTP server

`http.@serve(address :: Text, handler :: (Request) -> Response) -> net.Server` is a
compiler-lowered primitive, like `net.@tcpServe`: its lowering calls that very same runtime
entry, with `core.http`'s own connection handler filled in. Each accepted connection reads
a request head, reads a body-carrying method's own body (below), calls `handler` once, and
writes the reply — then, unless that request or reply ends the connection (below), reads
the next request off the same connection, looping for as long as the connection stays open.
A handler fault (a failing `assert`, an invalid index, …) is fatal, exactly as everywhere
else in the language, and takes the server with it. `kill` is `net.Server`'s own method
(see [`core.net`'s server layer](net.md#the-raw-tcp-server-layer)): `server.kill(seconds)`
or `server.kill()` for its 5-second default.

### Keep-alive

A connection carries more than one request when both sides are willing: `http.@serve`'s
own connection handler reads a request, answers it, and loops back to read the next one
off the same connection, carrying over any bytes a pipelined next request already sent
alongside the one just answered. The loop ends — the connection closes — the moment any of
these is true:

- The request's own `Connection` header says `close`.
- The reply `handler` returned carries its own `Connection: close` — an explicit header a
  handler sets itself; `Response.reply`'s own generated default (below) is `keep-alive`.
- The request line names `HTTP/1.0`: keep-alive is not that version's own default the way
  it is HTTP/1.1's, so a bare HTTP/1.0 request gets one response and a close.
- The next read off the connection returns no bytes at all — the peer closed, or
  `ServerOptions.idleTimeout` (below) passed with nothing arriving.

`Response.reply`'s own generated `Connection` header (below) is always `keep-alive`: the
constructor has no request to read a wish off, so it advertises the connection staying
open, the same default a modern HTTP/1.1 server uses absent a reason not to. What actually
closes the connection afterward is the loop above, not that header — a request that closes
still gets a reply carrying `Connection: keep-alive`, since the server intends to keep
every OTHER connection open and this response's own bytes are unaffected by why this one
particular connection is ending.

An idle connection — one with no request bytes arriving, whether waiting on a fresh request
or partway through one — closes once `ServerOptions.idleTimeout` seconds pass, the same
`Connection.@read(seconds)` timeout overload documented on [`core.net`](net.md#the-raw-tcp-server-layer)
does, given `idleTimeout` as its own deadline.

```quilon
<< core.http

hummus = (request :: http.Request) -> http.Response => <
  request.method ?
    | http.Get        => http.Response.reply(http.OK, "chickpeas: plenty")
    | http.Post(body) => http.Response.reply(http.Created, "stocked " + body.content)
    | _               => http.Response.reply(http.MethodNotAllowed)
>

^ = () -> Num => <
  shop = http.@serve("127.0.0.1:8080", request => hummus(request))
  ~ …
  shop.kill(5)
  0
>
```

`Request` gains the server side of `wire()`:

| Method | Result |
|--------|--------|
| `Request.parse(head :: Text, body :: Text) -> Result` | Parse a raw request head — everything up to, but not including, the blank line — into `Ok(Request)`: the request line's token to `Method`, its target to `url`, and the remaining lines through `Headers.parse`. `body` (`""` when the method carries none, or a caller has none to give) and the request's own `content-type` header (`""` when absent) are attached to a body-carrying method's own `Body`; a nullary method ignores both. `NotOk(reason)` when the request line carries fewer than two space-separated fields or names a method this module does not recognize — `serveConnection`'s own signal to answer 400 and close. |

`Request.path()` and `Request.params()` read a bare target the same way they read a full URL:
a target with no scheme or host (`/pantry?x=1`) has an authority of zero length, so the path
starts at its very first character.

### Request bodies

A body-carrying method (`Post`, `Put`, `Query`, `Patch`) whose request declares
`Content-Length` or `Transfer-Encoding: chunked` has its body read before `handler` runs:
`Content-Length: N` means exactly `N` more bytes past the head's blank line; `chunked` means
read until the zero-size chunk terminator, dechunked the same way the client's own
`Response.body()` dechunks a reply. A method that carries a body but whose request gives
neither header is called with an empty one — nothing is read.

The most bytes a body may carry is `ServerOptions.maxBodySize`, 16 MiB by default (the same
cap `net.@tcpRequest` already applies to a response). A declared `Content-Length` over the
cap is rejected the moment the head arrives, before any of the body itself has to; a
`chunked` body is rejected as soon as decoding it would cross the cap. Either way the
connection gets `413 Content Too Large` and closes, `handler` never called. Malformed
chunked framing (a non-hex chunk size, a missing chunk terminator, a chunk shorter than its
declared size) gets `400 Bad Request` instead — the same distinction `Request.parse`'s own
malformed-request-line case draws.

```quilon
<< core.http

hummus = (request :: http.Request) -> http.Response => <
  request.method ?
    | http.Post(body) => http.Response.reply(http.Created, "stocked " + body.content)
    | _               => http.Response.reply(http.MethodNotAllowed)
>

^ = () -> Num => <
  options = http.ServerOptions { maxBodySize = 1 * 1024 * 1024, idleTimeout = 5 }   ~ 1 MiB, down from 16
  shop = http.@serve("127.0.0.1:8080", request => hummus(request), options)
  ~ …
  shop.kill(5)
  0
>
```

| Type | Shape |
|------|-------|
| `ServerOptions` | `{ maxBodySize :: Num, idleTimeout :: Num }` — the request body cap, in bytes, and how many seconds a connection may sit idle before it closes (see [Keep-alive](#keep-alive)). |

| Method | Result |
|--------|--------|
| `ServerOptions.default() -> ServerOptions` | `{ maxBodySize = 16 * 1024 * 1024, idleTimeout = 5 }` (static) — 16 MiB and 5 seconds, the same idle default Node uses for its own server sockets. |
| `http.@serve(address, handler, options :: ServerOptions) -> net.Server` | As the two-argument form, with `options.maxBodySize`/`options.idleTimeout` in place of the defaults. |

`Response` gains one constructor, `reply`, over six overloads, plus `wire()`:

| Method | Result |
|--------|--------|
| `Response.reply(status :: Status) -> Response` | An empty-body reply carrying `status`. |
| `Response.reply(status :: Status, body :: Text) -> Response` | A reply carrying `status` and `body`. |
| `Response.reply(status :: Status, body :: Text, headers :: Headers) -> Response` | A reply carrying `status` and `body`, sending exactly `headers` alongside the generated ones. |
| `Response.reply(code :: Num) -> Response` | `reply(Status.parse(code))`. |
| `Response.reply(code :: Num, body :: Text) -> Response` | `reply(Status.parse(code), body)`. |
| `Response.reply(code :: Num, body :: Text, headers :: Headers) -> Response` | `reply(Status.parse(code), body, headers)`. |
| `wire() -> Text` | The reply's raw text (`it.raw`) — what `serveConnection` writes to the connection. |

Every constructor above sends **only** the headers it was given, plus two generated ones:
`content-length`, counted in bytes (`Text.size`), and `connection: keep-alive` — see
[Keep-alive](#keep-alive) for what actually decides whether the connection stays open. A
`content-type` comes from the three-argument overload — a program sets its own, the way it
sets any other header. `Response.reply(OK, "x")` sends exactly
`HTTP/1.1 200 OK\r\ncontent-length: 1\r\nconnection: keep-alive\r\n\r\nx`.

A program wanting a record literal built entirely by hand writes `http.Response { raw =
"..." }` directly, the same escape hatch the client side already offers.

Reading a request head off the wire is ordinary Quilon: locating the blank line is the same
grapheme-based search `Response.blankLine()` runs (all four spellings are ASCII), shared by
both as `blankLineIndex`. See `examples/http_server.qn` for a runnable stand.
