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
- **Hover** yields the inferred type of the smallest expression covering the cursor, from
  the type checker's table: `Num`, `[]Text`, `(Num) -> Num`, a record or sum type's name.
- **Semantic tokens** classify:
  - a `<` or `>` that delimits a block as a keyword token, and a `<` or `>` that is the
    comparison operator as an operator token;
  - an identifier matching one of the file's declared type names, function names, or
    parameter names as a type, function, or parameter token.
- **Code lens**: one lens above every `describe` (**▶ Run suite**) and every `it`
  (**▶ Run case**). The lens carries the client-side command `quilon.runTests` with two
  arguments — the file's path and the block's own `/`-joined path — so the command can run
  `quilon test <file> --only <path>` and scope to that suite or case.
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
