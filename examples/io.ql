~ Console output lives in the core.io module (imported explicitly).
~   print(x)   writes x + a newline to stdout (Num / Text / Bool)
~   eprint(x)  same, to stderr
~   write(content, fd)  raw bytes to a file descriptor; returns the byte count
~ `print(x)` is the ergonomic form of `x |> write(stdout)` plus the newline.
<< core.io

^ = () -> Num => <
  print("hello")            ~ stdout: hello\n
  "raw" |> write(stdout)    ~ stdout: raw   (no newline)
  eprint("done")            ~ stderr: done\n
  0
>
