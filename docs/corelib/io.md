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
| `print(x) -> $` | Write `x` to stdout **with a trailing newline**. Any type, rendered through its [`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-) — a `Bool` prints `True`/`False`; records, sum types, and arrays use their own member or the default for their shape. On output, text is [rendered for a reader](../types/text.md#string-interpolation-and-the-render-operator-): an invalid UTF-8 byte shows as `�`. Returns `$`. |
| `eprint(x) -> $` | Same, to stderr. Returns `$` (Unit). |
| `write(content, fd :: Num) -> Num` | Render `content` and write those bytes (no newline) to a file descriptor; returns bytes written. Byte-exact: a `Text` renders as itself, and the bytes go out as they are. |
| `@readStdin() -> Text` | Read one line from stdin (without the trailing newline). A [leaf IO primitive](../concurrency/README.md): it launches the read and returns a **deferred** `Text` forced on first strict use. Yields `""` at end-of-input. |
| `stdout`, `stderr` | The standard file descriptors. |

`print`, `eprint`, and `write` take **anything renderable**: the compiler resolves the
[`` ` `` render member](../types/text.md#string-interpolation-and-the-render-operator-) on the
argument's type, calls it, and writes the resulting `Text`. A type becomes printable by
defining that member — nothing extends `print`. A **function** value is the one thing with no
rendering, and printing one names the missing member.

The compiler claims these three names at their own arity (one argument for `print`/`eprint`,
two for `write`), so a definition there is rejected and points at the render member. A
definition at another arity is an ordinary [overload set](../functions/overloading.md) beside
the built-in.

```quilon
<< core.io
^ = () -> Num => <
  print("hello")            ~ stdout: hello\n
  write("raw", stdout)      ~ stdout: raw   (no newline)
  eprint("oops")            ~ stderr: oops\n
  0
>
```
There is no `println` — `print` owns the newline; `write` is the raw form. (See
`examples/io.qn`, and `examples/printing.qn` for printing a type of your own.)
