---
title: "core.io — I/O"
sidebar:
  label: "core.io"
  order: 1
---

# `core.io` — I/O

Import with `<< core.io`. See the [corelib index](README.md) and `examples/io.qn`.

| Function | Effect |
|----------|--------|
| `io.print(x) -> $` | Write `x` to stdout **with a trailing newline**. Any type, rendered through its [`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-) — a `Bool` prints `True`/`False`; records, sum types, and arrays use their own member or the default for their shape. On output, text is [rendered for a reader](../types/text.md#string-interpolation-and-the-render-operator-): an invalid UTF-8 byte shows as `�`. Returns `$`. |
| `io.eprint(x) -> $` | Same, to stderr. Returns `$` (Unit). |
| `io.write(content, fd :: Num) -> Num` | Render `content` and write those bytes (no newline) to a file descriptor; returns bytes written. Byte-exact: a `Text` renders as itself, and the bytes go out as they are. |
| `@readStdin() -> Text` | Read one line from stdin (without the trailing newline). A [leaf IO primitive](../concurrency/README.md): it launches the read and returns a **deferred** `Text` forced on first strict use. Yields `""` at end-of-input. |
| `@streamFile(path :: Text, chunkSize :: Num, onChunk :: (Text) -> Bool) -> Result` | Read `path` in `chunkSize`-byte reads, calling `onChunk` once per chunk. A [leaf IO primitive](../concurrency/README.md) that runs **strictly**, in program order on the calling fiber. `Ok(bytesRead)` / `NotOk(message)`. |
| `io.streamFile(path :: Text, onChunk :: (Text) -> Bool) -> Result` | `@streamFile` with a default chunk size the runtime chooses (64 KiB). |
| `io.stdout`, `io.stderr` | The standard file descriptors. |

`io.print`, `io.eprint`, and `io.write` take **anything renderable**: the compiler resolves
the [`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-)
on the argument's type, calls it, and writes the resulting `Text`. A type becomes printable
by defining that member. Every value renders except a **function** value; printing one is
a compile error naming the missing member.

`io.write`'s `fd` is a whole number of 0 or more — `io.stdout` and `io.stderr` always are.
A computed `fd` that at run time is anything else (NaN, an infinity, negative, a fraction,
or past what a 32-bit descriptor holds) fails loud at the call site with
[`QN508`](../tooling/errors.md#qn508--invalid-write-file-descriptor).

A write that reaches the operating system and fails there — a closed reader, a descriptor
naming no open file, or another I/O error — fails loud at the call site with
[`QN509`](../tooling/errors.md#qn509--write-failed); this holds for `io.print` and
`io.eprint` as well as `io.write`.

`@streamFile` delivers every chunk as whole, valid `Text`: an incomplete UTF-8 sequence or
grapheme cluster at the end of a read is held back and carried into the next one, so a chunk's
`.length` (grapheme count) is always correct and every chunk ends on a grapheme boundary —
even a flag emoji or a combining accent spanning a read boundary arrives whole, one read
later. A chunk holds at most `chunkSize` bytes plus a carried tail, and holds at least one
grapheme. `onChunk` returns `true` to keep reading or `false` to stop; stopping closes the
file at once, and reading ends there. It runs on the same fiber as its caller, in program
order — `onChunk` is the caller's own code, free to mutate a captured `:=` cell. The fiber
parks when a read reports not ready (a pipe or FIFO with nothing buffered yet); a regular
file's reads return at once. `Ok(bytesRead)` carries the total bytes delivered to `onChunk`,
at end-of-input or on a stop; `NotOk(message)` covers a missing file, a read error, invalid
UTF-8 in the file, a `chunkSize` that is zero, negative, or fractional, and a `chunkSize` too
large to allocate a chunk buffer for.

The module's names are reached through its binding, and its
[overload sets are closed](../modules/README.md#closed-overload-sets): a program's own bare
`print` or `write` — at any signature — is an unrelated function beside `io.print`.
(`@readStdin` and `@streamFile`, like every `@` primitive, keep their bare name once the
module is imported.)

```quilon
<< core.io
^ = () -> Num => <
  io.print("hello")            ~ stdout: hello\n
  io.write("raw", io.stdout)   ~ stdout: raw   (no newline)
  io.eprint("oops")            ~ stderr: oops\n
  0
>
```
`print` writes the newline; `write` is the raw form. (See `examples/io.qn`, and
`examples/printing.qn` for printing a user type.)
