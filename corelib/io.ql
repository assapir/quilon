~ core.io — output to file descriptors.
~ Import with `<< core.io`. Exposes:
~   write(content, fd)   ~ raw write of a Text to a file descriptor (no newline);
~                          returns the number of bytes written (Num)
~   print(x)             ~ write x to stdout, with a trailing newline (Num/Text/Bool)
~   eprint(x)            ~ same, to stderr
~   @readStdin()         ~ read one line from stdin (Text, without the trailing newline)
~   stdout, stderr       ~ the standard file descriptors (Num: 1 and 2)
~   colorEnabled(fd)     ~ Bool: is fd a terminal that wants ANSI color? (false when
~                          NO_COLOR is set, TERM=dumb, or fd is not a tty)
~
~ Examples:
~   << core.io
~   ^ = () -> Num => <
~     print("hello")              ~ prints: hello\n
~     print(42)                   ~ prints: 42\n
~     "raw" |> write(stdout)      ~ prints: raw   (no newline); == write("raw", stdout)
~     eprint("oops")              ~ to stderr, with a newline
~     0
~   >
~
~ These are part of the core library's public surface (exported with the `>>`
~ prefix) but are *compiler-lowered*: the code generator recognizes calls to
~ `print`/`eprint`/`write` and emits the matching runtime intrinsic (see
~ src/runtime/intrinsics.rs and CodeGenerator::generate_print / generate_write).
~ The function bodies below are inert placeholders; the lowering never emits them.
~ `stdout`/`stderr` are ordinary Num constants (file descriptors).

~ Standard output / error file descriptors.
>> stdout = 1
>> stderr = 2

~ Write a value to stdout followed by a newline. Polymorphic over Num / Text / Bool.
~ `print(x)` is the ergonomic form of `x |> write(stdout)` (plus the newline).
~ Returns Unit (`$`) — the printed value's "result" is meaningless.
>> print = x -> $ => $

~ Write a value to stderr followed by a newline. Polymorphic over Num / Text / Bool.
~ Returns Unit (`$`).
>> eprint = x -> $ => $

~ Write a Text's raw bytes to a file descriptor (no trailing newline).
~ Returns the number of bytes written. e.g. `"hi" |> write(stdout)`.
>> write = (content :: Text, fd :: Num) -> Num => 0

~ Read one line from stdin, returning it as a Text WITHOUT the trailing newline.
~ `@readStdin` is a leaf IO primitive (the `@` marker): calling it launches the read in the
~ background and hands back a DEFERRED Text immediately — the fiber only waits (forces) once
~ a strict operation reads the bytes (a comparison, `print`, a native call, ...). At
~ end-of-input it yields the empty Text `""`. The body below is an inert placeholder; the
~ code generator lowers `@readStdin()` to the runtime read/force intrinsics.
>> @readStdin = () -> Text => ""

~ Whether writing ANSI color to `fd` is appropriate: it is a terminal, `NO_COLOR` is
~ unset, and `TERM` is not `dumb`. Use it to style output only when a human is watching:
~   colorEnabled(stderr) ? "\e[1;31mboom\e[0m" : "boom"
>> colorEnabled = (fd :: Num) -> Bool => false
