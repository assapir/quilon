# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Quilon is a compiler for a statically-typed, symbol-based programming language (`.ql` files) that compiles to native code via LLVM. It is written in Rust and is in **early development** — many language features are partially implemented or stubbed. See `LANGUAGE.md` for the full language reference and the feature implementation matrix (✅ implemented / 🚧 partial / ❌ not yet).

## Build, check, test

```bash
cargo build              # debug build
cargo build --release    # release build (binary at target/release/quilon)
cargo test               # run all tests (lexer, integration, sum-type codegen/constructors)
cargo test test_name     # run a single test by name, e.g. cargo test test_result_pattern_extraction
cargo test --test integration_test   # run one test file
```

## Compiling and running .ql programs

**Important:** `quilon run` is declared in the CLI but **not implemented** (it's a `TODO` in `src/main.rs`). Only `compile` and `check` work. To produce and run a native executable you must drive LLVM manually:

```bash
cargo run -- check examples/hello_world.ql      # lex + parse + typecheck only
cargo run -- compile examples/hello_world.ql    # emits LLVM IR -> examples/hello_world.ll
llc -filetype=obj examples/hello_world.ll       # -> examples/hello_world.o
gcc examples/hello_world.o -o examples/hello_world   # link (note: clang is NOT installed here; use gcc)
./examples/hello_world; echo "Exit code: $?"
```

The value returned by a program's entry point becomes its exit code, which is how most examples verify behavior (e.g. `factorial(5)` → exit 120).

Every executable program **must** define a `>>` entry-point function (the compiler enforces this and auto-generates a C-compatible `main()` wrapper). Module imports (`<<`) are not implemented, so all programs are standalone.

## Compiler pipeline / architecture

The compiler is a classic multi-pass pipeline. `src/main.rs` wires the passes together for the `compile` and `check` subcommands; the same passes are exercised directly in the integration tests. Each stage is its own module under `src/`, re-exported through a `mod.rs`:

1. **Lexer** (`src/lexer/`, uses the `logos` crate) — source → tokens. `Lexer::tokenize(&str)`. Token kinds in `token.rs`.
2. **Parser** (`src/parser/ast_parser.rs`) — hand-written recursive-descent parser, tokens → AST. Entry: `parse(&tokens)`. This is the largest and most intricate file (~1600 lines) with ~17 precedence levels; `chumsky` is a dependency but the real parser is hand-written here.
3. **AST** (`src/ast/nodes.rs`) — `Program { items: Vec<Item> }`, where `Item::FunctionDecl`, etc. Pattern-matching and sum-type constructor nodes live here.
4. **Type checker** (`src/typechecker/checker.rs` + `inference.rs`) — `TypeChecker::new().check_program(&program)`. Does type inference, exhaustiveness checking, and arity checks.
5. **Code generator** (`src/codegen/generator.rs`, uses `inkwell` against **LLVM 21**) — `CodeGenerator::new(&context, module_name).generate(&program)` returns LLVM IR as a string.
6. **LLVM** (external `llc` + linker) — IR → native binary.

`src/runtime/` (parallel, io) is currently stubs for the planned implicit-parallelism / non-blocking-IO runtime.

## Things to know when changing the language

- A new language feature usually requires touching **all of**: lexer (new tokens), parser (`ast_parser.rs`), AST (`nodes.rs`), type checker (`checker.rs`), and codegen (`generator.rs`) — in that order. Tests in `tests/` follow the `tokenize → parse → check → generate` sequence and are the best template for end-to-end coverage.
- Numbers are a single unified `Num` type, represented internally as **f64** (and as such, array indices/discriminants are converted f64↔i64 in codegen).
- Arrays are `struct { ptr data, i64 size }` in LLVM — this backs both `.size` and `[index]`.
- Sum types (`Ok`/`NotOk`, the built-in `Result`) are tagged unions (i8 discriminant + payload). Constructor **codegen** and discriminant-based pattern matching are recently added and still incomplete in places (payloads are simplified to f64); custom user-defined sum types are not implemented. Check `LANGUAGE.md` and `tests/sum_*.rs` for current status before assuming a feature works end-to-end.
- The language has **no keywords** — syntax is symbol-based (`=`, `::`, `=>`, `->`, `< >` blocks, `>>` entry point, `?`/`|` pattern matching, `~` comments, `|>` pipeline). When editing the parser/lexer, consult the symbol table in `LANGUAGE.md`.

## Reference docs

- `LANGUAGE.md` — authoritative language reference, syntax, and the implemented/partial/not-yet feature list. Keep it in sync when you change language behavior.
- `README.md` — high-level vision (web-focused, implicit parallelism, deep immutability) — much is aspirational, not yet built.
- `examples/*.ql` — working programs referenced throughout `LANGUAGE.md`. The `.ll`/`.o`/binary files alongside them are build artifacts (gitignored).
