# Quilon

**A statically-typed, symbol-based language that compiles to native and should make me laugh. Colorless implicit futures on cooperative fibers — concurrency follows data dependence, not program order.**

Quilon (`.qn`) has no control-flow keywords — syntax is built from symbols (`^`, `<<`, `>>`, `|>`, `::`, `=>`, …). It targets native performance through LLVM with a small, unified type system.

> **Status: 0.9.1 — "stable basics."** The core language compiles, runs, and is tested end-to-end, but it is **not** feature-complete. For what is and isn't implemented, see the feature matrix in **[LANGUAGE.md](./docs/LANGUAGE.md)**.

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
  [1, 2, 3].each(n => print(n))   ~ iterate with array methods (no `for` loop)
  10 |> double                ~ ≡ double(10)
>
```

See **[LANGUAGE.md](./docs/LANGUAGE.md)** for the full reference — types, modules, pattern matching, I/O, the symbol table, and the feature matrix.

## Principles

- **Should make me laugh** — if a feature is a delight, that alone earns it a place.
- **Colorless concurrency** — implicit futures on cooperative fibers; no `async`/`await`.
- **Fail loud, never silent** — invalid operations error (at compile time when visible, else at run time); never a silent no-op, clamp, or magic sentinel.
- **Overloading, not generics** — ad-hoc overloading is the only polymorphism.
- **Eat the rich** — APIs expose everything up front; parsing and computing happen only when you touch it.

The full list lives in **[LANGUAGE.md](./docs/LANGUAGE.md#design-principles)**.

## Prerequisites

Install these **before** building or running Quilon:

- **LLVM 22** — the compiler backend (via inkwell). Debian/Ubuntu: [apt.llvm.org](https://apt.llvm.org); Arch: `llvm`; macOS: `brew install llvm@22`.
- **libgc (Boehm GC)** — the runtime GC. Needed to build the compiler, to `quilon run`, and at run time by `quilon build` binaries (dynamically linked), so it must be present wherever they run. Packages: `libgc-dev` (Debian/Ubuntu), `gc` (Arch), `bdw-gc` (Homebrew).
- **A C toolchain** — `clang` (default) or `gcc`, used by `quilon build` to link the executable. Not needed for `quilon run`.

The `quilon` binary is otherwise **self-contained**: `libquilon_rt.a` is embedded in it (gzip-compressed, unpacked to `~/.cache/quilon` on first build), so `quilon build` works from a bare download.

## Build & run

```bash
cargo build --release                        # binary at target/release/quilon
./target/release/quilon run   program.qn [args...]   # JIT-compile & execute (args pass through to the program, mirroring ./program args...)
./target/release/quilon build program.qn     # build a native executable (links libquilon_rt + libgc)
./target/release/quilon build program.qn --debug   # + DWARF line info, local variables & types for gdb/lldb source-level debugging (alias -g)
./target/release/quilon check program.qn     # typecheck only
```

Native builds link with `clang` by default; pass `--linker gcc` for gcc.

## Releasing

Run `./scripts/release.sh` from a clean `main`: it checks the `Cargo.toml`/`quilon-rt` versions and a dated `CHANGELOG.md` section, runs the full gate, then tags `v<version>` and pushes — which triggers CI to build and publish the GitHub release. Pass `--dry-run` to preview without tagging or pushing.

## Vision (aspirational)

Beyond 0.9, the design aims at **implicit parallelism** — sequential-looking code, parallel execution — and a **web-first** systems language. The runtime is single-threaded today; the parallel machinery is direction, not delivery.

## Licensing

**The Quilon compiler is free software under the GNU GPL v2** ([LICENSE.md](./LICENSE.md)) — forks and runtime modifications stay GPLv2.

**Programs you compile are yours to license however you want.** The runtime (`quilon-rt`) is statically linked into every binary `quilon build` produces, but it ships **GPLv2 _with_ a Classpath-style runtime-library exception** ([LICENSE-EXCEPTION.md](./LICENSE-EXCEPTION.md) — the model GCC and OpenJDK use), which also covers the runtime boilerplate the compiler emits (such as the generated C-compatible `main()`). So your output may be any license, including proprietary. The exception frees only the combined output; forking `quilon-rt` itself stays GPLv2.

Compiled binaries also link **libgc (the Boehm GC)**, a separate third-party dependency under its own permissive, MIT-style license.
