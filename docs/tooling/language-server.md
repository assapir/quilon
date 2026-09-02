---
title: "Language server"
---

# Language server

`quilon lsp` serves the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
over standard input and output. It is the compiler itself in a different frame: every
answer comes from a full run of the same front end (lex → parse → resolve `<<` imports →
type-check) that `quilon check` runs, over the text the editor holds — unsaved edits
included, since the client sends the buffer and the server treats it as the truth for that
file. Imported files are read from disk.

```bash
quilon lsp        # speaks the protocol on stdin/stdout; an editor starts this, not you
```

## Capabilities

- **Diagnostics** — published on open and on every change. The first front-end failure is
  reported at its span; an error inside an imported module is reported at the top of the
  open file, carrying the imported file's position in the message. A test file (top-level
  `describe` blocks and no `^` of its own) is checked with its blocks compiled — the same
  code `quilon test` runs — so errors inside test bodies surface too.
- **Go to definition** — on an identifier, the declaration that binds it: a parameter, a
  block-local binding, a pattern binding, a top-level function or type, or a declaration in
  an imported file (the result points into that file). A name from a bundled module
  (`core.io`, `core.test`, …) has no file on disk to open, and yields no result.
- **Hover** — the inferred type of the smallest expression covering the cursor, straight
  from the type checker's table: `Num`, `[]Text`, `(Num) -> Num`, a record or sum type's
  name.
- **Semantic tokens** — the classification only the compiler can make:
  - a `<` or `>` that delimits a block is reported as a keyword token, and a `<` or `>`
    that is the comparison operator as an operator token — the distinction a context-free
    grammar cannot draw, and the reason `< >` coloring needs the server;
  - identifiers matching the file's declared type names, function names, and parameter
    names are reported as type, function, and parameter tokens.
- **Code lens** — one lens above every `describe` (**▶ Run suite**) and every `it`
  (**▶ Run case**). The lens carries the client-side command `quilon.runTests` with the
  file's path as its argument; executing it is the editor's job (the Visual Studio Code
  extension runs `quilon test` on the file). Both lens kinds run the whole file's suites:
  the compiler runs suites per file and does not yet select a single suite or case.

Positions on the wire are the protocol's: zero-based lines and UTF-16 code-unit columns.
The server converts them to and from the compiler's byte spans, so multi-byte and
multi-grapheme text positions stay exact.

## Editor wiring

The [Visual Studio Code extension](https://github.com/assapir/quilon/tree/main/editors/vscode)
is a ready client: it locates the compiler (setting, `PATH`, install directories, a
checkout's build) and spawns `quilon lsp` itself.

Any other protocol client needs exactly one thing: start `quilon lsp` and speak the
protocol over its stdin/stdout. For Neovim:

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

## Scope

The server holds no state beyond the open documents' text; there is no cache and no
incremental analysis — each answer is one fresh front-end run, which the compiler's speed
makes affordable. Rename, find references, document symbols, and completion are future
capabilities of the same server.
