---
title: "Compiling & running"
---

# Compiling & running

Source files are **`.qn`**, and the compiler rejects a source named anything else. (Quilon
used `.ql` until 0.9.1; it is CodeQL's extension, so GitHub attributed Quilon programs to
CodeQL. Rename a `.ql` file to `.qn` — nothing else about it changes.)

```bash
quilon check   program.qn   # typecheck only — no code runs
quilon run     program.qn   # typecheck, then run directly (exit code = ^'s result)
quilon build   program.qn   # produce a native executable
quilon compile program.qn   # emit LLVM IR → program.ll (for inspection)
quilon test    [path]       # run the test suites under a file or directory (default: .)
```

`quilon --version` (or `-V`) prints the compiler version.

`quilon test` runs directly (like `quilon run`, no binary produced), and exits non-zero if
any case failed. It runs a file's top-level `describe` blocks, which every other command
erases — so tests may sit in the file they test, its `^` included, and still cost a release
build nothing. See [the test harness](../corelib/test/report.md).

`quilon build` produces a **self-contained** native executable — it runs on a machine with nothing else installed:
```bash
quilon build program.qn -o program       # default linker: clang
quilon build program.qn --linker gcc      # gcc also supported (CI checks both)
./program; echo "exit: $?"
```

Add `--debug` (or `-g`) to emit **DWARF debug info** for source-level debugging — a
debugger (`gdb`/`lldb`) can then set breakpoints, step, show backtraces in terms of
`.qn` lines, and **inspect local variables with their Quilon types**:
```bash
quilon build program.qn --debug -o program
llvm-dwarfdump --debug-line program        # lists the .qn file + its line table
llvm-dwarfdump --debug-info program        # shows variables + their debug types
gdb ./program                              # break/step by .qn line, print locals
```
Debug info is opt-in: without `--debug` the binary carries none. It covers line tables,
per-function scopes, and **locals, parameters, and debug types**. Every `=`/`:=` local and
parameter is emitted with its type, and nested `{ }` blocks and closures get their own
lexical scopes. Each Quilon type gets a distinct debug type — `Num`, `Bool`, `Text`,
arrays (`[]T`), records, and sum types — so a debugger tells them apart.
Line info is multi-file: a function from an imported module (`<<`) — corelib included — is
attributed to its OWN source, so a debugger steps into it. The entry frame reads `^`. A
debugger steps over the leaf `@` primitives and the built-ins (`print`/`now`/…).

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.qn`.)
