# `core.io` — I/O

Import with `<< core.io`. See the [corelib index](../LANGUAGE.md#corelib) and `examples/io.ql`.

| Function | Effect |
|----------|--------|
| `print(x) -> $` | Write `x` to stdout **with a trailing newline**. Any type, rendered through its [`` ` `` render operator](../LANGUAGE.md#string-interpolation-and-the-render-operator-) — a `Bool` prints `True`/`False`; records, sum types, and arrays use their default or overridden rendering. Returns `$`. A user `print` with a concrete signature *adds* an overload that wins for that type. |
| `eprint(x) -> $` | Same, to stderr. Returns `$` (Unit). |
| `write(content :: Text, fd :: Num) -> Num` | Write raw bytes (no newline) to a file descriptor; returns bytes written. |
| `@readStdin() -> Text` | Read one line from stdin (without the trailing newline). A [leaf IO primitive](../LANGUAGE.md#concurrency--colorless-implicit-futures--in-progress): it launches the read and returns a **deferred** `Text` forced on first strict use. Yields `""` at end-of-input. |
| `stdout`, `stderr` | The standard file descriptors. |

```quilon
<< core.io
^ = () -> Num => <
  print("hello")            ~ stdout: hello\n
  "raw" |> write(stdout)    ~ stdout: raw   (no newline)
  eprint("oops")            ~ stderr: oops\n
  0
>
```
There is no `println` — `print` owns the newline; `write` is the raw form. (See `examples/io.ql`.)
