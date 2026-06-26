~ core.io — basic console output.
~
~ Imported with `<< core.io`. `print`/`println` are part of the core library's
~ public surface (exported with the `>>` prefix) but are *compiler-lowered*: the
~ code generator recognizes calls to them and emits the matching runtime
~ intrinsic (__print_num/__println_num for Num, __print_cstr/__println_cstr for
~ Text) — see src/runtime/intrinsics.rs and CodeGenerator::generate_print.
~
~ They accept a Num or Text argument and return Num (0), so a print may be used
~ in expression position. The bodies below are inert placeholders; the lowering
~ never emits them.

~ Print a value without a trailing newline.
>> print = x => 0

~ Print a value followed by a newline.
>> println = x => 0
