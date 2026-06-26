~ core.io — console output.
~
~ Imported with `<< core.io`. These are part of the core library's public surface
~ (exported with the `>>` prefix) but are *compiler-lowered*: the code generator
~ recognizes calls to `print`/`eprint`/`write` and emits the matching runtime
~ intrinsic (see src/runtime/intrinsics.rs and CodeGenerator::generate_print /
~ generate_write). The function bodies below are inert placeholders; the lowering
~ never emits them. `stdout`/`stderr` are ordinary Num constants (file descriptors).

~ Standard output / error file descriptors.
>> stdout = 1
>> stderr = 2

~ Write a value to stdout followed by a newline. Polymorphic over Num / Text / Bool.
~ `print(x)` is the ergonomic form of `x :> write(stdout)` (plus the newline).
>> print = x => 0

~ Write a value to stderr followed by a newline. Polymorphic over Num / Text / Bool.
>> eprint = x => 0

~ Write a Text's raw bytes to a file descriptor (no trailing newline).
~ Returns the number of bytes written. e.g. `"hi" :> write(stdout)`.
>> write = (content :: Text, fd :: Num) -> Num => 0
