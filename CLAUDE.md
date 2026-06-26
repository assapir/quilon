# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repository.

## What this is

Quilon is a compiler for a statically-typed, **symbol-based** language (`.ql` files) that compiles to native code via LLVM, written in Rust. It is at **0.9 — "stable basics"**: the core works end-to-end (verified by run tests), but it is not feature-complete. **`LANGUAGE.md` is the authoritative reference and feature matrix** — consult it for what's implemented; don't duplicate that list here.

## Build, check, test

```bash
cargo build              # debug build
cargo build --release    # release build (binary at target/release/quilon)
cargo test               # full suite (lexer, parser, checker, codegen, module, run, sum)
cargo test test_name     # a single test by name
cargo test --test run_test   # one test file (e.g. the JIT exit-code tests)
```

Requires **LLVM 22** (for `inkwell`) and the system's **dynamic `libgc`** (Boehm GC) installed; CI installs `llvm-22-dev libpolly-22-dev libgc-dev`. (A static/vendored GC is a post-0.9 goal.)

**Strict CI:** the workflow fails on any warning — it runs `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo build`/`cargo test` under `RUSTFLAGS=-D warnings`. Keep changes warning-clean.

## Compiling & running `.ql` programs

All four subcommands share one front-end (`src/driver.rs`): read → lex → parse → resolve `<<` imports → typecheck.

```bash
cargo run -- check   examples/hello_world.ql   # front-end only
cargo run -- run     examples/hello_world.ql   # front-end + JIT execute (in-process LLVM)
cargo run -- build   examples/hello_world.ql   # native executable (see below)
cargo run -- compile examples/hello_world.ql   # emit LLVM IR -> .ll (for inspection)
```

`quilon run` is implemented (in-process JIT). A program's `^` entry point return value is its exit code (e.g. `factorial(5)` → 120) — this is how most run tests verify behavior. (The exit code is the `^` body's `Num` value, or 0 if the body isn't a `Num`.)

`quilon build` is a first-class Rust command (`src/build.rs`): it emits an object file in-process and links it with `libquilon_rt` (the runtime) and `libgc` into a native executable. `clang` is installed and is the **default** linker; `gcc` is also supported (CI checks both). There is no `scripts/aot.sh` and no manual `llc`/link step.

```bash
cargo run -- build examples/hello_world.ql -o hello       # default linker: clang
cargo run -- build examples/hello_world.ql --linker gcc
./hello; echo "exit: $?"
```

Every executable must define a `^` entry-point function (the compiler enforces this and generates a C-compatible `main()` that also initializes the GC).

## Compiler pipeline / architecture

Classic multi-pass pipeline; `src/driver.rs::front_end` wires the passes for the CLI, and tests exercise them directly. Stages (each a module under `src/`):

1. **Lexer** — `src/lexer/` (`logos`). `Lexer::tokenize(&str)`; token kinds in `token.rs`.
2. **Parser** — `src/parser/ast_parser.rs`, hand-written recursive descent, `parse(&tokens)`. The largest/most intricate file (~17 precedence levels).
3. **AST** — `src/ast/nodes.rs` — `Program { imports, items }`.
4. **Type checker** — `src/typechecker/` (`checker.rs` + `inference.rs`). Inference, exhaustiveness, arity.
5. **Code generator** — `src/codegen/generator.rs` (`inkwell`, **LLVM 22**) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting via `unicode-segmentation`, Boehm GC glue), packaged as `libquilon_rt`. Not stubs.
7. **Native / JIT** — `quilon build` (`src/build.rs`) emits an object in-process and links `libquilon_rt` + `libgc`; `quilon run` uses an in-process JIT (`src/jit.rs`).

## Things to know when changing the language

- A new feature usually touches **all of**: lexer (tokens), parser (`ast_parser.rs`), AST (`nodes.rs`), type checker (`checker.rs`), codegen (`generator.rs`) — in that order. Tests in `tests/` follow `tokenize → parse → check → generate → run`; the `run_test.rs` JIT harness asserting exit codes is the best end-to-end template.
- Numbers are one unified `Num` type (`f64`); array indices/discriminants convert f64↔i64 in codegen.
- Arrays and `Text` are both `{ ptr, i64 }` structs in LLVM (`Text` = `{ data, byte_len }`; arrays = `{ data, size }`). `Text` is a built-in type, no import.
- Sum types (`Ok`/`NotOk`) are tagged unions (i8 tag + payload). **Numeric payloads work; non-numeric data in composites — `Text` as a payload (`Ok(text)`), or `Text` inside a record/array — doesn't type-check yet** — check `LANGUAGE.md` "Known limitations" and `tests/sum_*.rs` before assuming.
- **No keywords** — symbol-based: `^` entry point, `<<` import, `>>` export, `|>` pipe (first-arg injection: `x |> f(a)` ≡ `f(x, a)`), `for n <- collection => body` loops, `?`/`|`/`_` pattern matching, `? :` ternary, `~` comments. Consult the symbol table in `LANGUAGE.md`.
- I/O lives in the `core.io` module (`<< core.io`): `print`/`eprint`/`write`/`stdout`/`stderr`. There is no `println`. `print`/`eprint` are compiler-lowered builtins (polymorphic over Num/Text/Bool).

## Reference docs

- `LANGUAGE.md` — authoritative language reference, syntax, and the ✅/🚧/❌ feature matrix. Keep it in sync when you change language behavior.
- `README.md` — high-level pitch + aspirational vision (implicit parallelism, deep immutability — not yet built).
- `examples/*.ql` — runnable programs referenced from `LANGUAGE.md`; each is exercised by the test suite. The `.ll`/`.o`/binary artifacts alongside them are gitignored.
