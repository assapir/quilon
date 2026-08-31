# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repository.

## What this is

Quilon is a compiler for a statically-typed, **symbol-based** language (`.qn` files) that compiles to native code via LLVM, written in Rust. It is at **0.9 — "stable basics"**: the core works end-to-end (verified by run tests), but it is not feature-complete. **The `docs/` tree (starting at `docs/README.md`) is the authoritative language reference, and `docs/status/feature-matrix.md` the feature matrix** — consult them for what's implemented; don't duplicate that list here.

**Planning & process docs (read these when working toward 1.0):**
- **`docs/ROADMAP.md`** — the authoritative plan: milestone roadmap (M1–M7 + status) and the locked language-design decisions (1–20). What's decided, done, and next. Do not relitigate a locked decision without asking the user.
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
4. **Type checker** — `src/typechecker/checker.rs` plus its per-area child modules (assertions, errors, env, overloads, sums, decls, exprs, calls, patterns). Inference, exhaustiveness, arity.
5. **Code generator** — `src/codegen/generator.rs` plus its per-area child modules (arrays, assertions, calls, closures, decls, di, exprs, interpolation, intrinsics, mangle, matching, oracle, records, sums, tco, text) (`inkwell`, **LLVM 22**) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting via `unicode-segmentation`, Boehm GC glue), packaged as `libquilon_rt` — which bundles the collector's object too, so the archive alone is a complete runtime. Not stubs.
7. **Native / JIT** — `quilon build` (`src/build.rs`) emits an object in-process and links `libquilon_rt`; `quilon run` uses an in-process JIT (`src/jit.rs`).

## Things to know when changing the language

- A new feature usually touches **all of**: lexer (tokens), parser (`ast_parser.rs`), AST (`nodes.rs`), type checker (`checker.rs`), codegen (`generator.rs`) — in that order. Tests in `tests/` follow `tokenize → parse → check → generate → run`; the `run_test.rs` JIT harness asserting exit codes is the best end-to-end template.
- Numbers are one unified `Num` type (`f64`); array indices/discriminants convert f64↔i64 in codegen.
- Arrays and `Text` are both `{ ptr, i64 }` structs in LLVM (`Text` = `{ data, byte_len }`; arrays = `{ data, size }`). `Text` is a built-in type, no import.
- Sum types (`Ok`/`NotOk`) are tagged unions (i8 tag + payload). `Num`/`Bool`/`Text` payloads all work end-to-end (`Ok(text)`), as does `Text` (and nested arrays) inside records/arrays — a pattern-bound payload carries its concrete type. What is **not** unified: different payload *types* in the same slot position across variants (e.g. `Num` in one variant, `Text` in another) is rejected — check `docs/status/limitations.md` and `tests/sum_*.rs` before assuming.
- **No keywords** — symbol-based: `^` entry point, `<<` import, `>>` export, `|>` pipe (first-argument injection: `x |> f(a)` ≡ `f(x, a)`), `?`/`|`/`_` pattern matching, `? :` ternary, `~` comments. There is **no loop construct** — iterate with array methods (`.each`/`.map`/`.filter`/`.reduce`) and recursion (self-tail-calls are lowered to loops). Consult the symbol table in `docs/README.md`.
- **Modules are qualified** (see `docs/modules/README.md`): `<< core.http` binds `http`, and exports are reached through the binding — `http.send(...)`, `http.Request { }`, `| http.Get =>` — with the full path (`core.http.send`) as the ambiguity escape. Implemented as a link-time rename pass (`src/ast/qualify.rs`, driven by `src/modules.rs`): imported items — privates included, which travel but resolve for no importer — take fully-qualified names, so the checker and codegen see one flat namespace. Imports claim their short names (a binding named `http` after the import is an error); `@` primitives stay bare after import; module overload sets are **closed** (a user's bare `print`/`now` is an unrelated function).
- **Assertions are compiler-provided** and stay bare: `assert(value, matcher)` (fatal, exit 101) and `expect(value, matcher)` (recorded, only inside a `test.it` case) over the matchers `equals`/`contains`/`not`/`isOk`/`isNotOk` — no import needed. Neither the entry points nor the matchers are written in `.qn` — a matcher holds a value of the type under test, which without generics would need a matcher type per type. Checked in `src/typechecker/checker/assertions.rs`, lowered in `src/codegen/generator/assertions.rs`. The harness lives in `corelib/test.qn` (`core.test`): `test.describe`/`test.it` and the `core.test.reportSummary` the synthesized test entry point calls, over `test.failAt`, the run's recorded state, and the case lifecycle. Its report is fixed — the colors and the per-group/per-case lines are written out inside those functions rather than behind exported helpers. See `docs/corelib/test/`.
- I/O lives in the `core.io` module (`<< core.io`), reached through its binding: `io.print`/`io.eprint`/`io.write`/`io.stdout`/`io.stderr`. There is no `println`. `io.print`/`io.eprint`/`io.write` take **anything renderable**: the compiler resolves the `` ` `` render member on the argument's type, calls it, and writes the resulting `Text` (the same path string interpolation takes). A type becomes printable by defining that member — nothing extends `print`. `core.time`'s `now` is reached as `time.now()`; all of these are still compiler-lowered intrinsics underneath (the corelib bodies are inert placeholders).
- **Overloading is ad-hoc and explicit** (the only polymorphism — no generics): 2+ same-named top-level FUNCTION definitions with fully annotated signatures (every parameter **and** the return type) form an overload set; calls resolve by **exact** static argument types (no coercion). **Operator** overloading lives INSIDE the type: an operator (`+`, `==`, …) is a **member** of the record or sum it operates on, with `it` the left operand and the one explicit parameter the right (a top-level operator definition is rejected). A sum may carry a trailing `{ }` block of methods (methods only, no fields). Built-in operators (`+`, comparisons including `Text`) and `core.time`'s `now` go through this same overload mechanism. Codegen mangles each member to a distinct symbol. `>` lexes as block-close by default; it is the greater-than operator only when an operand follows it on the same line.
- **No hoisting**: names resolve top to bottom, so a call only sees definitions above it (a definition is in scope for its own body, so self-recursion works; mutual recursion between top-level functions is not expressible). Overload members join their set as their definition is reached.

## Reference docs

- `docs/README.md` + its subfolders — authoritative language reference and syntax; `docs/status/feature-matrix.md` — the ✅/🚧/❌ feature matrix. Keep them in sync when you change language behavior.
- `README.md` — high-level pitch + aspirational vision (implicit parallelism, deep immutability — not yet built).
- `examples/*.qn` — runnable programs referenced from the language reference under `docs/`; each is exercised by the test suite. The `.ll`/`.o`/binary artifacts alongside them are gitignored.
