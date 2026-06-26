# Quilon for VS Code

Syntax highlighting and editor tasks for the [Quilon](../../README.md) programming
language — a statically-typed, **symbol-based** language (no keywords) that
compiles to native code via LLVM. Files use the `.ql` extension.

## Features

- **Syntax highlighting** for the full symbol set:
  - Comments (`~ to end of line`), strings (`"…"` with escapes), numbers, `true`/`false`, wildcard `_`.
  - The entry point `^`, module import `<<` (with the imported path), and export marker `>>`.
  - Operators: `|>` (pipe), `:=` (mutable bind) vs `=` (immutable bind), `::` (type annotation),
    `=>` (function body / match arm), `->` (return type), `<-` (loop iterate),
    `?` / `|` (pattern matching), arithmetic `+ - * / %`, comparison `== != < <= > >=`,
    logical `&& || !`.
  - Built-in types `Num` / `Text` / `Bool`.
  - **Capitalized identifiers** are highlighted as types / sum-type constructors
    (`Ok`, `NotOk`, `Color`, `Circle`); **lowercase** names followed by `(` as function calls.
- **Bracket matching & auto-closing** for `< >`, `{ }`, `[ ]`, `( )`, and `"`.
- **Editor tasks & commands** to run the compiler on the active file.

## Install / run locally

This extension is not published to the Marketplace. To try it from this checkout:

### Option A — Extension Development Host (recommended)

1. Open the `editors/vscode/` folder in VS Code.
2. Press `F5` ("Run Extension"). A new "Extension Development Host" window opens.
3. Open any `.ql` file (e.g. one from `examples/`) — highlighting is active.

### Option B — install as a `.vsix`

```bash
cd editors/vscode
npm install -g @vscode/vsce   # if you don't have vsce
vsce package                  # produces quilon-0.1.0.vsix
code --install-extension quilon-0.1.0.vsix
```

### Option C — symlink into your extensions folder

```bash
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/quilon-0.1.0
```

Reload VS Code afterwards.

## Running the compiler from the editor

Two commands are contributed (open the Command Palette, `Ctrl/Cmd+Shift+P`):

- **Quilon: Check Current File** → runs `quilon check <file>`
- **Quilon: Run Current File** → runs `quilon run <file>`

They run in an integrated terminal named "Quilon". By default they invoke a
`quilon` binary on your `PATH`. If you are working from a checkout of the
compiler instead, set:

```jsonc
// settings.json
"quilon.command": "cargo run --"
```

The bundled `.vscode/tasks.json` also provides **quilon: check current file**
and **quilon: run current file** tasks (`Terminal → Run Task…`).

> The `quilon run` subcommand must exist in your toolchain. Depending on your
> build it may instead be `compile` + manual `llc`/link — see the repo's
> `CLAUDE.md` / `LANGUAGE.md`.

## Diagnostics & debugging

- **No inline diagnostics (squiggles) yet.** The compiler currently reports
  errors using **byte offsets**, e.g.
  `❌ Type error: Type mismatch at Span { start: 42, end: 47 }`, not
  `file:line:column`. A VS Code problem matcher needs line/column to place a
  diagnostic, so errors are surfaced in the terminal output instead of as
  editor squiggles. Mapping byte spans → ranges is a natural future enhancement
  (e.g. via a small language server).
- **No step debugging.** Real breakpoint/step debugging requires DWARF debug
  info, which the Quilon compiler does not emit yet — so this extension does
  **not** ship a debug adapter. The `.vscode/launch.json` contains only a
  trivial run-only configuration. A proper debugger is a future feature.

## License

See [`LICENSE.md`](../../LICENSE.md) at the repo root.
