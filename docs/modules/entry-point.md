---
title: "Entry point"
---

# Entry point

Every executable defines `^` (main); the program starts there.
```quilon ignore
^ = () -> Num => 42                              ~ no args/env
^ = (args :: []Text) -> Num => args.size         ~ command-line arguments
^ = (args :: []Text, env :: [|Text => Text|]) -> Num => env.get("HOME")   ~ args + environment
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
the `.qn` path becomes `argv[0]`. So `quilon run f.qn a b c` gives the same `args.size`
and trailing arguments as a native `./f a b c` — `argv[0]` is the `.qn` path rather than
the compiled binary's path, but everything the program indexes past it matches. An
argument or environment entry containing a NUL byte is refused by `run`, exactly as the
operating system refuses to start a native binary with one. Any other `^` signature (e.g.
a non-`Text` array element, or an unexpected parameter) is a compile-time error, reported
by `check` as well as `run`/`build`.

**Exit code:** if `^`'s body evaluates to a `Num`, that value is the exit code. If the body is **not** a `Num` (e.g. a side-effecting block), the program exits **0** — so an effect-only `main` needs no trailing `0`. (This implicit-0 applies only to `^`; ordinary functions always return their last expression's value.)

(See `examples/hello_world.qn` and `examples/args.qn`.)
