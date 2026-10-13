---
title: "Runtime errors"
sidebar:
  order: 5
---

# Runtime errors

QN5xx: failures a compiled program raises while running.

## Runtime

### QN500 — assertion failed

An `assert` or `expect` found its value outside what the matcher accepts. `assert` exits
5; `expect` marks the running case failed and continues.

```quilon ignore
^ = () -> $ => < assert(2 + 2, equals(5)) >
```

The message states what was expected and what was found; fix the program or the
expectation.

### QN501 — index out of bounds

An `array[i]` read has `i` below 0, at or past the array's size, or a fraction.

```quilon ignore
^ = () -> Num => < a = [1, 2]  a[5] >
```

Index within `0` and `a.size - 1`, or check `i < a.size` first.

### QN502 — fractional or unrepresentable range endpoint

A computed `lo <- hi` endpoint is a fraction, NaN, infinite, or beyond the whole numbers a
`Num` holds exactly.

```quilon ignore
^ = () -> Num => <
  half = 0.5
  (1 <- half).size
>
```

Compute whole-number endpoints. (The range expression is written on its own line here — a
`(` that instead followed `half = 0.5` on the SAME line would open a call on `half`, per
[the statement-boundary rule](../../expressions/README.md).)

### QN503 — no arm matched

A `?` match reached the end of its arms. The checker proves every match exhaustive; this is
the runtime's backstop.

```text
error[QN503]: no arm of this match matched the value
```

File the program as an issue.

### QN504 — allocation failed

The collector had no memory for the request, or the requested size was outside what a
`Num` represents.

```quilon ignore
^ = () -> Num => <
  xs = 1 <- 9007199254740992
  xs.size
>
```

Allocate within the machine's memory, and compute sizes that stay whole numbers.

### QN505 — reading stdin failed

`@readStdin` met an IO error on stdin — one other than end of file.

```text
error[QN505]: core.io.@readStdin failed: Input/output error (os error 5)
```

Run the program with a readable stdin.

### QN506 — empty `from` in `replace`/`replaceAll`

A computed `from` argument to `Text.replace` or `Text.replaceAll` was the empty text at
run time — an empty `from` is an ill-defined request. (A literal empty `from` is instead a
compile-time error.)

```quilon ignore
^ = () -> Num => <
  from = ""
  "abc".replaceAll(from, "x").size
>
```

Pass a non-empty `from`.

### QN507 — stack overflow

Recursion deep enough to exhaust a fiber's stack hits its guard page. The report carries
no source location — the frame that would have named one is itself the exhausted call.

```quilon ignore
deep = (n :: Num) -> Num => < n == 0 ? 0 : 1 + deep(n - 1) >
^ = () -> Num => < deep(10000000) >
```

Recurse fewer levels, or write the recursion as a self-tail-call — `n == 0 ? 0 : deep(n -
1)` — which is lowered to a loop and runs in constant stack.

### QN508 — invalid write file descriptor

A computed `fd` argument to `io.write` was, at run time, something other than a whole
number of 0 or more — NaN, an infinity, negative, a fraction, or past what a 32-bit
descriptor holds. (`io.stdout`/`io.stderr` are always whole and non-negative, so this is
reached only through a descriptor the program computes itself.)

```quilon ignore
<< core.io

^ = () -> Num => <
  nonsenseFd = 0 / 0
  io.write("kazoo manifesto", nonsenseFd)
>
```

Pass a whole descriptor of 0 or more.

### QN509 — write failed

A write that reached the operating system failed there — a closed reader (`EPIPE`), a
descriptor that names no open file (`EBADF`), or another I/O error the write call reported.
An interrupted write (`EINTR`) is retried transparently and never reaches here; `write`,
`print`, and `eprint` all report through this code.

```quilon ignore
<< core.io

^ = () -> Num => < io.write("smoke signal, delayed", 99) >
```

Pass a descriptor that names an open file, or a stream something is still reading.

### QN510 — invalid `replace` count

A computed `count` argument to `Text.replace` was, at run time, either less than 1 once
truncated toward zero, or greater than the occurrences of `from` present in the
receiver. A literal violation of either is a compile-time error.

```quilon ignore
^ = () -> Num => <
  zeroCount = 3 - 3
  "a-a-a".replace("a", "b", zeroCount).size
>
```

Pass a whole `count` of 1 or more, no greater than the occurrences of `from` present.

### QN511 — bind failed

`core.net.@tcpServe` could not resolve or bind the requested address (`host:port`, the
same form `net.@tcpRequest` accepts) — the address does not parse or resolve, the port is
missing or out of range, the port is already in use, or nothing but a privileged process
may bind it. The report names the address exactly as written. Starting a server is the
one point in the raw TCP layer where failure is fatal; every per-connection failure
afterward stays inside the running server instead.

```quilon ignore
<< core.net

^ = () -> Num => <
  first = net.@tcpServe("127.0.0.1:47990", connection => $)
  second = net.@tcpServe("127.0.0.1:47990", connection => $)
  0
>
```

Pick an address nothing else on the machine is already bound to.

### QN512 — mismatched listener transport and identity at startup

A `Listener` whose `transport` and `identity` disagree — `TLS` with `identity = None`,
or `Plain` with an identity other than `None` — reached `core.net.@tcpServe`/
`http.@serve`'s listener-list form at run time (built some way other than a literal
written in place, which [QN359](semantics.md#qn359--mismatched-listener-transport-and-identity)
catches at compile time instead). The report names the listener's address and which
side of the disagreement it is on.

```quilon ignore
<< core.net

mismatched = (transport :: net.Transport, identity :: net.Identity) -> net.Listener => <
  net.Listener { address = ":443", transport = transport, identity = identity }
>

^ = () -> Num => <
  server = net.@tcpServe([mismatched(net.TLS, net.None)], connection => $)
  0
>
```

Give a `TLS` listener a `Files`/`Base64` identity, or give a `Plain` listener `None`.

### QN513 — TLS identity failed to load

A `TLS` listener's identity could not be turned into a certificate and a private key:
`Files`'s path named no readable file, `Base64`'s text did not decode, the decoded or
read bytes were not a PEM certificate chain or private key in an accepted encoding
(PKCS#8, SEC1, or PKCS#1), or the key did not match the certificate. The report names
the listener's address and a reason, exactly as a bind failure
([QN511](#qn511--bind-failed)) does.

```quilon ignore
<< core.net

^ = () -> Num => <
  server = net.@tcpServe(
    [net.Listener {
      address = ":443", transport = net.TLS,
      identity = net.Files("missing-cert.pem", "missing-key.pem")
    }],
    connection => $)
  0
>
```

Point `Files`/`Base64` at a certificate chain (leaf first) and a matching private key.

### QN514 — address() on a server with more than one listener

`Server.address()` reports one bound address, which only a server with exactly one
listener has. A server `net.@tcpServe`/`http.@serve` started from a listener LIST, with
more than one entry, has no single address to report — `Server.addresses()` reports
every listener's bound address instead.

```quilon ignore
<< core.net

^ = () -> Num => <
  server = net.@tcpServe(
    [
      net.Listener { address = "127.0.0.1:0", transport = net.Plain, identity = net.None },
      net.Listener { address = "127.0.0.1:0", transport = net.Plain, identity = net.None }
    ],
    connection => $)
  server.address().port
>
```

Call `addresses()` instead, and pick the entry for the listener in question.
