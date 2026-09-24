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
