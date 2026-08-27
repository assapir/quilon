---
title: "core.io — I/O"
sidebar:
  order: 1
---

# `core.io` — I/O

Import with `<< core.io`. See the [corelib index](README.md) and `examples/io.qn`.

| Function | Effect |
|----------|--------|
| `print(x) -> $` | Write `x` to stdout **with a trailing newline**. Any type, rendered through its [`` ` `` render operator](../types/text.md#string-interpolation-and-the-render-operator-) — a `Bool` prints `True`/`False`; records, sum types, and arrays use their default or overridden rendering. On output, text is [rendered for a reader](../types/text.md#string-interpolation-and-the-render-operator-): an invalid UTF-8 byte shows as `�`. Returns `$`. |
| `eprint(x) -> $` | Same, to stderr. Returns `$` (Unit). |
| `write(content :: Text, fd :: Num) -> Num` | Write raw bytes (no newline) to a file descriptor; returns bytes written. Byte-exact: the bytes as they are. |
| `@readStdin() -> Text` | Read one line from stdin (without the trailing newline). A [leaf IO primitive](../concurrency/README.md): it launches the read and returns a **deferred** `Text` forced on first strict use. Yields `""` at end-of-input. |
| `stdout`, `stderr` | The standard file descriptors. |

`print`, `eprint`, and `write` are [overload sets](../functions/overloading.md) — defining one
with another signature adds a member rather than shadowing the built-in.

```quilon
<< core.io
^ = () -> Num => <
  print("hello")            ~ stdout: hello\n
  "raw" |> write(stdout)    ~ stdout: raw   (no newline)
  eprint("oops")            ~ stderr: oops\n
  0
>
```
There is no `println` — `print` owns the newline; `write` is the raw form. (See `examples/io.qn`.)
