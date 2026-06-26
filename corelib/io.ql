~ core.io — output to file descriptors.
~ Import with `<< core.io`. Exposes:
~   write(content, fd)   ~ raw write of a Text to a file descriptor (no newline);
~                          returns the number of bytes written (Num)
~   print(x)             ~ write x to stdout, with a trailing newline (Num/Text/Bool)
~   eprint(x)            ~ same, to stderr
~   stdout, stderr       ~ the standard file descriptors (Num: 1 and 2)
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
>> print = x => 0

~ Write a value to stderr followed by a newline. Polymorphic over Num / Text / Bool.
>> eprint = x => 0

~ Write a Text's raw bytes to a file descriptor (no trailing newline).
~ Returns the number of bytes written. e.g. `"hi" |> write(stdout)`.
>> write = (content :: Text, fd :: Num) -> Num => 0
