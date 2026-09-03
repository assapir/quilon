---
title: "Compiling & running"
---

# Compiling & running

Source files are **`.qn`**; the compiler rejects a source with any other extension.

```bash
quilon check   program.qn   # typecheck only — no code runs
quilon run     program.qn   # typecheck, then run directly (exit code = ^'s result)
quilon build   program.qn   # produce a native executable
quilon compile program.qn   # emit LLVM IR → program.ll (for inspection)
quilon test    [path]       # run the test suites under a file or directory (default: .)
quilon test    suite.qn --reporter json          # one JSON event per line, for a tool
quilon test    suite.qn --only "Suite/case name" # run one suite or case (repeatable)
quilon test    suite.qn --binary out             # build a debuggable executable instead of running
quilon lsp                  # serve the Language Server Protocol over stdin/stdout (see language-server.md)
```

`quilon --version` (or `-V`) prints the compiler version and its release codename.
`quilon explain QN311` prints the reference section for an error code (see
[Error messages](errors.md)).

## Status output

Every command reports its status on **stderr**, beside diagnostics; stdout carries the
program's own output and what a command is asked to produce (a
[test run's report](../corelib/test/README.md), IR, a rendered explanation). On an
**interactive terminal**, `check`, `build`, and `compile` show the stages (lexing, parsing,
resolving, checking, generating, linking) as one live line that collapses, on success, into
a single closing line — the file, the elapsed time, and a quip:

```text
✓ examples/hello_world.qn (9ms) — no keywords were harmed
```

The live line draws over itself and clears before the closing line prints; the closing
line is what scrollback keeps. Off a terminal — a pipe, a redirected log, or a CI run (a
terminal check, or `CI` set in the environment) — stderr carries the closing line alone.
`quilon run` writes the program's output; a failure is reported the same way everywhere.

- `--quiet` (`-q`, before or after the subcommand) silences status lines. Diagnostics
  print.
- `NO_COLOR` set to a non-empty value, `TERM=dumb`, or a redirected stderr keeps every line
  plain.
- `QUILON_QUIP_SEED=<n>` pins which quip each line carries, making a run reproducible end
  to end. `quilon test`'s closing line and the multi-suite tally carry one too.

`quilon test` runs directly, like `quilon run`, and exits non-zero when any case failed. It
runs a file's top-level `describe` blocks, which every other command erases — tests may sit
in the file they test, its `^` included. A suite's progress is its file heading and case
tree, from
[the test harness](../corelib/test/README.md#selecting-cases-and-choosing-the-reporter); on
an interactive terminal a single transient "compiling `file`" line stands in while a suite's
front end runs, clearing before its case tree prints. `--reporter json` prints one JSON
object per event on stdout and stays silent otherwise — a plain stream for a tool, on a
terminal too.

`quilon build` produces a **self-contained** native executable that runs on a machine with the operating system alone:
```bash
quilon build program.qn -o program       # default linker: clang
quilon build program.qn --linker gcc      # gcc also supported (CI checks both)
./program; echo "exit: $?"
```

`quilon test <file> --binary <out>` builds the suite into a **native, debuggable
executable** at `<out>` instead of running it — always with DWARF debug info, so a debugger
(`gdb`/`lldb`) can step through a case. `<file>` names one suite; a directory (the default
`.` included) is a diagnostic error, since a build has one entry point. `--only`, given
alongside `--binary`, is baked into the executable: the build drops every `describe`/`it`
the selection excludes before code generation, so running `<out>` alone reproduces the
filtered run — the shape a debugger's launch configuration wants.
```bash
quilon test suite.qn --binary suite_debug
quilon test suite.qn --only "Suite/one case" --binary suite_debug   # only that case is built in
gdb ./suite_debug
```
The executable's exit code follows `quilon test`'s own convention: 0 when every case that
ran passed.

Add `--debug` (or `-g`) to emit **DWARF debug info** for source-level debugging — a
debugger (`gdb`/`lldb`) can then set breakpoints, step, show backtraces in terms of
`.qn` lines, and **inspect local variables with their Quilon types**:
```bash
quilon build program.qn --debug -o program
llvm-dwarfdump --debug-line program        # lists the .qn file + its line table
llvm-dwarfdump --debug-info program        # shows variables + their debug types
gdb ./program                              # break/step by .qn line, print locals
```
Debug info is opt-in through `--debug`. It covers line tables,
per-function scopes, and **locals, parameters, and debug types**. Every `=`/`:=` local and
parameter is emitted with its type, and nested `{ }` blocks and closures get their own
lexical scopes. Each Quilon type gets a distinct debug type — `Num`, `Bool`, `Text`,
arrays (`[]T`), records, and sum types — so a debugger tells them apart.
Line info is multi-file: a function from an imported module (`<<`) — corelib included — is
attributed to its OWN source, so a debugger steps into it. The entry frame reads `^`. A
debugger steps over the leaf `@` primitives and the built-ins (`io.print`/`time.now`/…).

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.qn`.)
