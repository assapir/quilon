# Compiler architecture

A classic multi-pass pipeline (each stage a module under `src/`); `src/driver.rs` runs the shared front-end (read → lex → parse → resolve imports → typecheck) for all CLI commands and renders any failure through `src/diagnostic.rs` (the rustc-style `path:line:col` reporter described under [Error messages](../tooling/errors.md)).

1. **Lexer** — `src/lexer/` (`logos`), `Lexer::tokenize(&str)`.
2. **Parser** — `src/parser/ast_parser.rs`, hand-written recursive descent, `parse(&tokens)`.
3. **AST** — `src/ast/nodes.rs`.
4. **Type checker** — `src/typechecker/checker.rs` plus its per-area child modules.
5. **Code generator** — `src/codegen/generator.rs` plus its per-area child modules (`inkwell`, LLVM 22) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting, GC glue), packaged as `libquilon_rt`.
7. **LLVM** — `quilon build` emits an object in-process and links `libquilon_rt` into a native binary; `quilon run` uses an in-process JIT.

See `CLAUDE.md` for contributor guidance.
