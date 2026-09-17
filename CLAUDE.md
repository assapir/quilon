# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repository.

## What this is

Quilon is a compiler for a statically-typed, **symbol-based** language (`.qn` files) that compiles to native code via LLVM, written in Rust. It is at **0.11**: the language and its toolchain work end-to-end (verified by run tests), but it is not feature-complete. **The `docs/` tree (starting at `docs/README.md`) is the authoritative language reference, and `docs/status/feature-matrix.md` the feature matrix** — consult them for what's implemented; don't duplicate that list here.

**Planning & process docs (read these when working toward 1.0):**
- **`docs/ROADMAP.md`** — the authoritative plan: milestone roadmap (M1–M7 + status). Design decisions are recorded in the language reference under `docs/` (see ROADMAP.md's own pointer near the top), and ROADMAP's milestone tables mark each one that is locked. What's decided, done, and next. Do not relitigate a locked decision without asking the user.
- **`docs/ORCHESTRATION.md`** — how Quilon is built with a multi-agent workflow, and the hard rules: **no merge to `main` without explicit per-PR user approval**; **any design decision → stop and ask the user**; every feature ships docs + tests + a wired-in example; `/code-review` + `/simplify` before commit; worktree discipline; parallelize independent work.

## Build, check, test

```bash
cargo build              # debug build
cargo build --release    # release build (binary at target/release/quilon)
cargo test               # full suite (lexer, parser, checker, codegen, module, run, sum)
cargo test test_name     # a single test by name
cargo bench              # both benchmark families (compile speed, and generated-code speed + latency)
cargo test --test run_test   # one test file (e.g. the JIT exit-code tests)
```

Requires **LLVM 22** (for `inkwell`) and a C compiler; CI installs `llvm-22-dev libpolly-22-dev`. The Boehm GC is a **git submodule** (`quilon-rt/vendor/bdwgc`, pinned to a release tag) that `quilon-rt/build.rs` compiles via the `cc` crate and links statically, so there is no libgc to install and a binary `quilon build` produces runs on a machine that has none. Clone with `--recurse-submodules`, or run `git submodule update --init` — without it the build stops with that instruction.

Two families, both reading **committed** corpora so every run measures the same programs,
both printing tables and asserting nothing; CI publishes them to the job summary, where a
regression shows up as a column growing over time.

- `cargo bench --bench compile_speed` — the compiler: lex / parse / link / check /
  codegen per corpus in `benches/corpus/`, plus the run's peak RSS.
- `cargo bench --bench runtime_speed` — what the compiler *emits*: wall time and peak RSS
  of the built programs in `benches/runtime/`, then `quilon run` / `quilon build` latency
  including a cold runtime-archive cache.

Both families take `--baseline <path>` (a previous run's numbers) and `--metrics <path>`
(where to record this run's). With a baseline, each table gains a `Δ` column — locally that
is `cargo bench --bench compile_speed -- --metrics before.tsv`, then the same with
`--baseline before.tsv` after a change. CI does this across runs on a branch (the series is
kept in `actions/cache`, restored by prefix), so the job summary shows a delta against the
previous run on the same branch. It is **informational**: shared runners are noisy in
absolute terms and only interleaved runs on one machine compare credibly, so nothing gates
on a delta and a few percent either way is the floor. No baseline (first run, evicted
cache, a fork) prints the tables exactly as before. Format: one tab-separated
`family<TAB>row<TAB>metric<TAB>value` row per measurement, in `benches/series.rs` and gated
by `tests/bench_series_test.rs`.

`cargo bench --bench <name> -- --regen` rewrites that family's corpora from the
generators in the bench. Resizing a corpus is a deliberate act that lands as a reviewable
diff and breaks comparability with earlier numbers, which is why it is not automatic. Add
a corpus when a change has a cost profile the existing ones don't cover.

**Strict CI:** the workflow fails on any warning — it runs `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo build`/`cargo test` under `RUSTFLAGS=-D warnings`. Keep changes warning-clean.

## Compiling & running `.qn` programs

Every subcommand shares one front-end (`src/driver.rs`): read → lex → parse → resolve `<<` imports → typecheck.

```bash
cargo run -- check   examples/hello_world.qn   # front-end only
cargo run -- run     examples/hello_world.qn   # front-end + JIT execute (in-process LLVM)
cargo run -- build   examples/hello_world.qn   # native executable (see below)
cargo run -- compile examples/hello_world.qn   # emit LLVM IR -> .ll (for inspection)
cargo run -- test    examples/test_suite.qn    # run a test suite (JIT only; default path: .)
```

`quilon test` runs a file's top-level `test.describe(...)` blocks (under `<< core.test`)
with a synthesized `^`; every other subcommand erases them, so tests may live in the file
they test — and with nothing left referencing it, the `<< core.test` those blocks needed is
tree-shaken out of the build by `src/ast/reachability.rs`. `src/test_command.rs` is the
runner; `docs/corelib/test/` is the reference.

`quilon run` is implemented (in-process JIT). A program's `^` entry point return value is its exit code (e.g. `factorial(5)` → 120) — this is how most run tests verify behavior. (The exit code is the `^` body's `Num` value, or 0 if the body isn't a `Num`.)

`quilon build` is a first-class Rust command (`src/build.rs`): it emits an object file in-process and links it with `libquilon_rt` (the runtime, which carries the statically built GC) into a native executable. `clang` is installed and is the **default** linker; `gcc` is also supported (CI checks both). There is no `scripts/aot.sh` and no manual `llc`/link step.

```bash
cargo run -- build examples/hello_world.qn -o hello       # default linker: clang
cargo run -- build examples/hello_world.qn --linker gcc
./hello; echo "exit: $?"
```

Every executable must define a `^` entry-point function (the compiler enforces this and generates a C-compatible `main()` that also initializes the GC).

## Compiler pipeline / architecture

Classic multi-pass pipeline; `src/driver.rs::front_end` wires the passes for the CLI, and tests exercise them directly. Stages (each a module under `src/`):

1. **Lexer** — `src/lexer/` (`logos`). `Lexer::tokenize(&str)`; token kinds in `token.rs`.
2. **Parser** — `src/parser/ast_parser.rs`, hand-written recursive descent, `parse(&tokens)`. The largest/most intricate file (~17 precedence levels).
3. **AST** — `src/ast/nodes.rs` — `Program { imports, items }`.
4. **Type checker** — `src/typechecker/checker.rs` plus its per-area child modules (assertions, errors, env, overloads, sums, declarations, exprs, calls, patterns). Inference, exhaustiveness, arity.
5. **Code generator** — `src/codegen/generator.rs` plus its per-area child modules (arrays, assertions, calls, closures, declarations, di, exprs, interpolation, intrinsics, mangle, matching, oracle, records, sums, tco, text) (`inkwell`, **LLVM 22**) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting via `unicode-segmentation`, Boehm GC glue), packaged as `libquilon_rt` — which bundles the collector's object too, so the archive alone is a complete runtime. Not stubs.
7. **Native / JIT** — `quilon build` (`src/build.rs`) emits an object in-process and links `libquilon_rt`; `quilon run` uses an in-process JIT (`src/jit.rs`).

## Things to know when changing the language

- A new feature usually touches **all of**: lexer (tokens), parser (`ast_parser.rs`), AST (`nodes.rs`), type checker (`checker.rs`), codegen (`generator.rs`) — in that order. Tests in `tests/` follow `tokenize → parse → check → generate → run`; the `run_test.rs` JIT harness asserting exit codes is the best end-to-end template.
- **Numbers** (`Num` = `f64`) — `docs/types/README.md`. Array indices/discriminants convert f64↔i64 in codegen.
- **`Text`/arrays** (`Text = []Grapheme`, native vs. corelib-composable methods) — `docs/types/text.md`, `docs/collections/arrays.md`; LLVM struct shapes in `docs/status/abi.md`. Merged in by `src/modules.rs::link`; codegen lowers a composable member call to the module's qualified function (`core.text.trim`).
- **Sum types** (`Ok`/`NotOk`, tagged unions) — `docs/types/sum-types.md`; ABI layout in `docs/status/abi.md`. Check `tests/sum_*.rs` before assuming payload-typing edge cases.
- **A body is a block** (a lambda's may be a bare expression instead) — `docs/expressions/README.md`, `docs/functions/README.md`. Enforced in `Parser::parse_body` (`src/parser/ast_parser/items.rs`).
- **No keywords, no loop construct** — symbol table in `docs/README.md`; iteration model in `docs/expressions/iteration.md`; self-tail-call lowering in `docs/functions/closures.md`.
- **Modules are qualified** — `docs/modules/README.md`. Implemented as a link-time rename pass (`src/ast/qualify.rs`, driven by `src/modules.rs`): imported items take fully-qualified names, so the checker and codegen see one flat namespace.
- **Assertions are compiler-provided** — `docs/corelib/test/README.md`. Checked in `src/typechecker/checker/assertions.rs`, lowered in `src/codegen/generator/assertions.rs`. Harness in `corelib/test.qn`; the report's colors and per-group/per-case lines are written out inline rather than behind exported helpers.
- **I/O** — `docs/corelib/io.md`; `core.time`'s `now` — `docs/corelib/time.md`. Both (and the assertions/test harness) are compiler-lowered intrinsics underneath; the corelib bodies are inert placeholders.
- **Overloading is ad-hoc and explicit** (no generics) — `docs/functions/overloading.md`; a sum's trailing method block — `docs/types/sum-types.md#methods--the-optional---block`; `>` as block-close vs. greater-than — `docs/expressions/README.md`. Codegen mangles each member to a distinct symbol.
- **No hoisting** — `docs/functions/README.md#names-resolve-top-to-bottom`, `docs/variables.md`, `docs/functions/overloading.md`.

## Reference docs

- `docs/README.md` + its subfolders — authoritative language reference and syntax; `docs/status/feature-matrix.md` — the ✅/🚧/❌ feature matrix. Keep them in sync when you change language behavior.
- `README.md` — high-level pitch + aspirational vision (implicit parallelism, deep immutability — not yet built).
- `examples/*.qn` — runnable programs referenced from the language reference under `docs/`; each is exercised by the test suite. The `.ll`/`.o`/binary artifacts alongside them are gitignored. An example that reads stdin gets its own input from a sidecar file, `examples/<name>.stdin`, piped to it on every path the gate runs.
