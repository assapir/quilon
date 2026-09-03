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
| `io.stdout`, `io.stderr` | The standard file descriptors. |

`io.print`, `io.eprint`, and `io.write` take **anything renderable**: the compiler resolves
the [`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-)
on the argument's type, calls it, and writes the resulting `Text`. A type becomes printable
by defining that member. Every value renders except a **function** value; printing one is
a compile error naming the missing member.

The module's names are reached through its binding, and its
[overload sets are closed](../modules/README.md#closed-overload-sets): a program's own bare
`print` or `write` — at any signature — is simply an unrelated function beside `io.print`.
(`@readStdin`, like every `@` primitive, keeps its bare name once the module is imported.)

```quilon
<< core.io
^ = () -> Num => <
  io.print("hello")            ~ stdout: hello\n
  io.write("raw", io.stdout)   ~ stdout: raw   (no newline)
  io.eprint("oops")            ~ stderr: oops\n
  0
>
```
There is no `println` — `print` owns the newline; `write` is the raw form. (See
`examples/io.qn`, and `examples/printing.qn` for printing a type of your own.)
