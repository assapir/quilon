---
title: "Entry point"
---

# Entry point

Every executable defines `^` (main); the program starts there.
```quilon ignore
^ = () -> Num => < 42 >                          ~ no args/env
^ = (args :: []Text) -> Num => < args.size >     ~ command-line arguments
^ = (args :: []Text, env :: [|Text => Text|]) -> Num => < env.get("HOME") > ~ args + environment
```
**Arguments & environment.** `^` may declare, in order, two typed parameters, filled at
startup:
- `args :: []Text` — the command-line arguments (argv), **including** `argv[0]` (the
  program name), so `args.size` is always at least 1, and `args[i]` is the *i*-th
  argument as a `Text`.
- `env :: [|Text => Text|]` — the environment, as a Map from each variable's name to its
  value. An entry `KEY=value` is split on its **first** `=` (so `KEY=a=b` maps `KEY` to
  `a=b`); an entry with no `=` maps the whole string to `""`. Read a variable with
  `env.get("HOME")` (or `<< core.cli`'s `getEnv`), both giving `Ok(value)`/`NotOk`.

`args` is a real Quilon array (`.size`, `[index]`, the array methods) and `env` a real Map
(`.get`/`.has`/`.keys`/`.size`). A value read out of either is a full `Text`: the whole
`Text` API, and [overload](../functions/overloading.md) dispatch by its concrete type.

`quilon run <file> [args...]` and a native build agree on `args`. Under `run`, the
program sees `argv = [<file>, <args...>]`: the `quilon`/`run` CLI prefix is stripped and
the `.qn` path becomes `argv[0]`. `quilon run f.qn a b c` gives the same `args.size`
and trailing arguments as a native `./f a b c`; `argv[0]` is the `.qn` path under `run`
and the binary's path in a native run. An
argument or environment entry containing a NUL byte is refused by `run`, exactly as the
operating system refuses to start a native binary with one. Any other `^` signature (e.g.
a non-`Text` array element, or an unexpected parameter) is a compile-time error, reported
by `check` as well as `run`/`build`, and exits with its own code's family digit (see
[Exit codes](../tooling/errors.md#exit-codes)) rather than the convention below.

**Exit code:** when `^`'s body evaluates to a `Num`, that value becomes the exit code by
converting it to a whole number, clamping it to the 32-bit signed range first, then letting
the operating system keep only its low 8 bits — the usual process-exit convention. NaN and
negative infinity give **0**; positive infinity gives **255**. `__exit(code)` (the native
primitive `assert`'s failure and `core.test`'s harness end with) converts its own `code`
the same way. A body of any other type (e.g. a side-effecting block) exits **0**; an
effect-only `main` ends without a trailing `0`. (The implicit 0 applies to `^`; an ordinary
function returns its last expression's value.) A fail-loud runtime check — a failing
`assert`, a bad `array[i]`, a broken pipe — bypasses this convention too, exiting 5
regardless of what `^` would otherwise have returned (see [Exit
codes](../tooling/errors.md#exit-codes)).

(See `examples/hello_world.qn` and `examples/args.qn`.)
