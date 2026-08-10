~ Console output lives in the core.io module (imported explicitly).
~   print(x)   writes x + a newline to stdout (Num / Text / Bool)
~   eprint(x)  same, to stderr
~   write(content, fd)  raw bytes to a file descriptor; returns the byte count
~ `print(x)` is the ergonomic form of `x |> write(stdout)` plus the newline.
~ `<< core.test` verifies the byte count `write` returns; on success the program exits 0.
<< core.io
<< core.test

^ = () -> $ => <
  print("hello")            ~ stdout: hello\n
  written :: Num = "raw" |> write(stdout)   ~ stdout: raw   (no newline)
  assertEq(written, 3)      ~ "raw" is 3 bytes
  eprint("done")            ~ stderr: done\n
>
