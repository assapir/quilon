---
title: "Language server"
---

# Language server

`quilon lsp` serves the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
over standard input and output. Each answer is one full run of the compiler front end
(lex → parse → resolve `<<` imports → type-check) over the open document's text as the
editor last sent it. Imported files are read from disk.

```bash
quilon lsp        # speaks the protocol on stdin/stdout; an editor starts it
```

## Capabilities

- **Diagnostics** are published on open and on every change. A front-end failure is
  reported at its span. A failure inside an imported module is reported at the top of the
  open document, with the imported file's position in the message. A test file (top-level
  `describe` blocks and no `^`) is checked with its blocks compiled — the code `quilon test`
  runs — so a failure inside a test body is reported.
- **Go to definition** on an identifier yields the declaration that binds it: a
  parameter, a block-local binding, a pattern binding, a top-level function or type, or a
  declaration in an imported file (the location points into that file). A name declared in
  a bundled module (`core.io`, `core.test`, …) yields no location.
- **Find references** on an identifier — or on the name in its own declaration — yields
  every place that binds or reads it: a parameter's declaration and every use in its
  function, a top-level function's or type's declaration (every member, for an overload
  set) and every use across the document, a block-local's or pattern binding's declaration
  and every use in its own scope. Both cover the names an identifier binds: parameters,
  block-locals, pattern bindings, and top-level functions and types.
- **Rename** on the same targets as find references rewrites the declaration and every use
  in one edit. The new name must be a single bare identifier; a target declared in another
  file answers with a message naming that file, so the rename happens there instead.
- **Hover** yields the inferred type of the smallest expression covering the cursor, from
  the type checker's table: `Num`, `[]Text`, `(Num) -> Num`, a record or sum type's name.
- **Completion** (triggered on `.`, and answered on every request regardless of what
  triggered it) offers, depending on where the cursor sits:
  - **A bare name.** Locals and parameters of the enclosing blocks (only bindings ABOVE
    the cursor — Quilon has no hoisting), the document's top-level functions and types
    defined above the cursor (never the enclosing definition itself), every sum type's
    constructors defined above the cursor, and every `<<` import's binding.
  - **After `binding.` for an imported module** (`http.`): that module's exported names,
    the same qualified names `http.Response` or `http.Get` reach — resolved by loading
    that one import in isolation, so this answers even when the rest of the document does
    not parse.
  - **After `expr.` for any other expression** (`response.`): the checked receiver
    type's members — a record's fields and methods, a sum's methods, or the fixed
    built-in members of `Text`, an array, a `Map`, or a `Set`.

  Each item carries a protocol `kind` (variable, function, field, method, class, module,
  or enum member) and a `detail` string with the type or signature, in the same spelling
  hover uses. A completion request's document is normally unparseable at the cursor
  (`response.` has no member yet); rather than keep a second, cached copy of the
  last-clean document around, the server re-derives a checkable document from the CURRENT
  buffer by deleting just the incomplete token at the cursor — the trailing `.member`
  being typed, or the bare word being typed. What is left parses (and, but for the module
  case, checks) like any other snapshot, one token earlier. The one trade-off: a cursor
  inside an expression that is ALREADY broken for an unrelated reason (a stray paren
  earlier in the file) still answers empty — there is no fallback to a stale good version.
- **Semantic tokens** classify:
  - a `<` or `>` that delimits a block as a keyword token, and a `<` or `>` that is the
    comparison operator as an operator token;
  - an identifier matching one of the file's declared type names, function names, or
    parameter names as a type, function, or parameter token.
- **Code lens**: two lenses above every `describe` (**▶ Run suite** / **🐞 Debug suite**)
  and every `it` (**▶ Run case** / **🐞 Debug case**). Both carry the same two arguments —
  the file's path and the block's own `/`-joined path — through a client-side command:
  `quilon.runTests` runs `quilon test <file> --only <path>` in place, scoped to that suite
  or case; `quilon.debugTests` builds `quilon test <file> --only <path> --binary <out>` and
  launches the result under a debugger instead.
- **`quilon/testItems`** (custom request, `{ textDocument: { uri } }` → an array) answers
  the same test tree as the code lenses, flat: one entry per suite and case, in document
  order, each an object with `path` (the names from the outermost `describe` down, joined
  by `/` — the path [`quilon test --only`](../corelib/test/README.md#paths) expects),
  `name` (the suite's or case's own description), `kind` (`"suite"` or `"case"`), and
  `range` (its `describe(...)`/`it(...)` call, in protocol positions). A client building a
  test explorer reads this request for its tree.

Positions on the wire are the protocol's: zero-based lines and UTF-16 code-unit columns.
The server converts them to and from the compiler's byte spans.

## Editor wiring

The [Visual Studio Code extension](https://github.com/assapir/quilon/tree/main/editors/vscode)
locates the compiler and spawns `quilon lsp` itself.

Any other protocol client starts `quilon lsp` and speaks the protocol over its
stdin/stdout. For Neovim:

```lua
vim.lsp.config("quilon", {
  cmd = { "quilon", "lsp" },
  filetypes = { "quilon" },
  root_markers = { ".git" },
})
vim.lsp.enable("quilon")
```

For Helix (`languages.toml`):

```toml
[language-server.quilon]
command = "quilon"
args = ["lsp"]

[[language]]
name = "quilon"
scope = "source.quilon"
file-types = ["qn"]
language-servers = ["quilon"]
```
