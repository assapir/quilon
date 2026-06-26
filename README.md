# Quilon

**A statically-typed, symbol-based language that compiles to native code via LLVM.**

Quilon (`.ql`) has no control-flow keywords — syntax is built from symbols (`^`, `<<`, `>>`, `|>`, `::`, `=>`, …). It targets native performance through LLVM with a small, unified type system.

> **Status: 0.9.0 — "stable basics."** The core language works and is verified end-to-end (it compiles, runs, and is tested). It is **not** feature-complete. For exactly what is and isn't implemented, see the feature matrix in **[LANGUAGE.md](./LANGUAGE.md)**.

## A taste

```quilon
<< core.io

~ functions are arrow bindings
double = x => x * 2

~ Text is a built-in type: + concatenates, .length counts graphemes
greet = name :: Text => "Hello, " + name

~ the pipe |> injects the left value as the first argument
^ = () -> Num => <
  print(greet("Quilon"))      ~ stdout: Hello, Quilon
  for n <- [1, 2, 3] => print(n)
  10 |> double                ~ ≡ double(10)
>
```

See **[LANGUAGE.md](./LANGUAGE.md)** for the full reference (types, modules, pattern matching, I/O, the symbol table, and the feature matrix).

## Build & run

```bash
cargo build --release            # binary at target/release/quilon
cargo run -- run   program.ql    # JIT-compile and execute (exit code = the program's result)
cargo run -- build program.ql    # build a native executable (links libquilon_rt + libgc)
cargo run -- check program.ql    # typecheck only
```

Requires LLVM 22 and `libgc` (Boehm GC) installed. Contributor and architecture notes are in **[CLAUDE.md](./CLAUDE.md)**.

## Vision (aspirational)

The longer-term goals that motivate the design — **not all implemented in 0.9**:

- **Implicit parallelism** — sequential-looking code, parallel execution.
- **Deep immutability** — immutable by default, enabling fearless parallelism.
- **No function coloring** — non-blocking I/O without `async`/`await`.
- **Web-first** — a systems-level language aimed at high-performance web services.

Today these are direction, not delivered features; the runtime is single-threaded and the parallel/non-blocking machinery is not built yet.

## License

GPL-2.0.
