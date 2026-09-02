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
quilon test    suite.qn --reporter json          # one JSON event per line, for a tool
quilon test    suite.qn --only "Suite/case name" # run one suite or case (repeatable)
```

`quilon --version` (or `-V`) prints the compiler version and its release codename.
`quilon explain QN311` prints the reference section for an error code (see
[Error messages](errors.md)).

## Status output

Every command reports its progress on **stderr**, apart from diagnostics and the program's
own output; stdout carries only what a command is actually asked to produce (a
[test run's report](../corelib/test/README.md), IR, a rendered explanation). On an
**interactive terminal**, `check`, `build`, and `compile` show the stages (lexing, parsing,
resolving, checking, generating, linking) as one live line that collapses, on success, into
a single closing line — the file, the elapsed time, and a quip:

```text
✓ examples/hello_world.qn (9ms) — no keywords were harmed
```

That live line is the ONLY place a stage ever appears: it draws over itself and clears
before the closing line prints, so scrollback never carries one. Off a terminal — a pipe, a
redirected log, or a CI run (detected from the terminal check alone, or from `CI` set in the
environment) — stage progress is silent altogether, and stderr is the closing line alone.
`quilon run` prints nothing of its own beyond the program's output; a failure is reported
the same way everywhere.

- `--quiet` (`-q`, before or after the subcommand) prints no status at all. Diagnostics
  still print.
- `NO_COLOR` set to a non-empty value, or `TERM=dumb`, keeps every line plain; so does a
  redirected stderr.
- `QUILON_QUIP_SEED=<n>` pins which quip each line carries, so a run is reproducible end
  to end. `quilon test`'s closing line and the multi-suite tally carry one too.

`quilon test` runs directly (like `quilon run`, no binary produced), and exits non-zero if
any case failed. It runs a file's top-level `describe` blocks, which every other command
erases — so tests may sit in the file they test, its `^` included, and still cost a release
build nothing. It never shows per-stage compile progress for a suite, even on a terminal —
a suite's own file heading and case tree, from
[the test harness](../corelib/test/README.md#selecting-cases-and-choosing-the-reporter), are
the progress that matters; on an interactive terminal a single transient "compiling `file`"
line stands in while a suite's front end runs, clearing before its case tree prints.
`--reporter json` prints one JSON object per event on stdout and nothing else — no status
line, no quip, no color, ever, even on a terminal — for a tool reading the stream.

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
debugger steps over the leaf `@` primitives and the built-ins (`io.print`/`time.now`/…).

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.qn`.)
