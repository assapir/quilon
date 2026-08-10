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
  [1, 2, 3].each(n => print(n))   ~ iterate with array methods (no `for` loop)
  10 |> double                ~ ≡ double(10)
>
```

See **[LANGUAGE.md](./LANGUAGE.md)** for the full reference (types, modules, pattern matching, I/O, the symbol table, and the feature matrix).

## Prerequisites

Install these **before** building or running Quilon:

- **LLVM 22** — the compiler backend (via inkwell). Debian/Ubuntu: install from [apt.llvm.org](https://apt.llvm.org); Arch: `llvm`; macOS: `brew install llvm@22` (or the current `llvm`).
- **libgc (Boehm GC)** — the runtime garbage collector, and a hard dependency: it is required to **build** the `quilon` compiler, to **`quilon run`** (the JIT resolves `GC_*` in-process), **and** it is **dynamically linked into the native executables** produced by `quilon build` — so libgc must also be present wherever those binaries run. Packages: `libgc-dev` (Debian/Ubuntu), `gc` (Arch), `bdw-gc` (Homebrew).
- **A C toolchain** — `clang` (default) or `gcc`, plus `llc` (ships with LLVM). Used by `quilon build` to assemble and link native executables. Not needed for `quilon run` (JIT).

A compiled `quilon` binary is otherwise **self-contained**: the runtime static
library (`libquilon_rt.a`) is embedded, gzip-compressed, in the binary itself,
so `quilon build` works from a bare binary download (e.g. a GitHub release)
with no extra files — the first build decompresses the runtime into
`$XDG_CACHE_HOME/quilon` (default `~/.cache/quilon`); later builds reuse that
cached copy. Only the system libraries above (notably libgc) must be installed.

## Build & run

```bash
cargo build --release                        # binary at target/release/quilon
./target/release/quilon run   program.ql [args...]   # JIT-compile & execute (args pass through to the program, mirroring ./program args...)
./target/release/quilon build program.ql     # build a native executable (links libquilon_rt + libgc)
./target/release/quilon check program.ql     # typecheck only
```

`cargo build --release` is all you need: `quilon build` locates the runtime
static library (`libquilon_rt.a`) automatically — a `QUILON_RT_LIB` environment
variable override, then a copy next to the `quilon` binary (the build script
places one there), then the compressed copy embedded in the binary (see
Prerequisites) — no extra step. Native builds link with `clang` by default;
pass `--linker gcc` to use gcc instead.

## Vision (aspirational)

The longer-term goals that motivate the design — **not all implemented in 0.9**:

- **Implicit parallelism** — sequential-looking code, parallel execution.
- **Deep immutability** — immutable by default, enabling fearless parallelism.
- **No function coloring** — non-blocking I/O without `async`/`await`.
- **Web-first** — a systems-level language aimed at high-performance web services.

Today these are direction, not delivered features; the runtime is single-threaded and the parallel/non-blocking machinery is not built yet.

## Licensing

**Quilon the compiler is free software under the GNU GPL, version 2** (see
[LICENSE.md](./LICENSE.md)). If you fork or modify the compiler — or the runtime
library — that work stays GPLv2. The copyleft is intact.

**Programs you compile with Quilon are yours to license however you want.** The
Quilon runtime (`quilon-rt`) is statically linked and embedded into every binary
`quilon build` produces, and normally GPL code linked into your program would
pull the whole thing under the GPL. To prevent that, `quilon-rt` is
**GPLv2 _with_ a Classpath-style runtime-library exception** (see
[LICENSE-EXCEPTION.md](./LICENSE-EXCEPTION.md), the same model GCC and OpenJDK
use). The exception also covers the runtime boilerplate the compiler emits into
your output (such as the generated C-compatible `main()` wrapper). So the mere
presence of these runtime bits does **not** place your compiled program under
the GPL — you may release it under any license, including proprietary ones.

The exception is an *additional grant on top of* GPLv2 and frees only the
combined output. Forking `quilon-rt` itself remains GPLv2.

Compiled programs also link **libgc (the Boehm GC)**, a separate third-party
dependency under its own permissive, MIT-style license. It is not covered by,
and does not need, the exception; its own license applies to it.
