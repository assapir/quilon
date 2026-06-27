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
  - Built-in types `Num` / `Text` / `Bool`, and the unit type/value `$` (`$` is
    both the type, as in `-> $`, and its sole value — highlighted like the other
    built-in types).
  - **Capitalized identifiers** are highlighted as types / sum-type constructors
    (`Ok`, `NotOk`, `Color`, `Circle`); **lowercase** names followed by `(` as function calls.
- **Bracket matching & auto-closing** for `< >`, `{ }`, `[ ]`, `( )`, and `"`.
- **Inline diagnostics** — type/parse/lex errors from the compiler appear as
  editor squiggles (see [Diagnostics & debugging](#diagnostics--debugging)).
- **Editor tasks & commands** to run the compiler on the active file.

## Install / run locally

The extension is written in **TypeScript** (`src/extension.ts`), compiled to
`out/extension.js` by `tsc`. Install dependencies once before building or
debugging:

```bash
cd editors/vscode
npm install
```

This extension is not published to the Marketplace. To try it from this checkout:

### Option A — Extension Development Host (recommended)

1. Open the `editors/vscode/` folder in VS Code.
2. Press `F5` ("Run Extension"). The `compile` preLaunchTask builds the
   TypeScript, then a new "Extension Development Host" window opens.
3. Open any `.ql` file (e.g. one from `examples/`) — highlighting is active.

Use `npm run watch` for incremental recompiles while iterating.

### Option B — install as a `.vsix`

```bash
cd editors/vscode
npm install                   # if you haven't already
npm run package               # compiles + produces quilon-0.1.0.vsix (via vsce)
code --install-extension quilon-0.1.0.vsix
```

## Development

The extension is TypeScript (strict). It is linted with **oxlint** and formatted
with **oxfmt** (the [Oxc](https://oxc.rs) toolchain) — not ESLint/Prettier:

```bash
npm run compile     # tsc: type-check + emit out/extension.js
npm test            # compile, then run the unit tests (node --test)
npm run lint        # oxlint (fails on any finding)
npm run lint:fix    # oxlint --fix (auto-fix what it can)
npm run fmt         # oxfmt --write (format in place)
npm run fmt:check   # oxfmt --check (verify formatting; CI gate)
```

CI runs `lint`, `fmt:check`, `compile`, `test`, and `package` on every PR that
touches `editors/vscode/**` (see [Publishing](#publishing)).

### Tests & manual verification

Unit tests cover the diagnostic-output parser (`src/diagnostics.ts`), which is
kept free of any `vscode` import so it runs under plain Node. To verify the
**inline diagnostics** end-to-end manually:

1. Set `quilon.command` to a working compiler (e.g. `"cargo run --"` from a
   checkout, or `"quilon"` if it's on your `PATH`).
2. Launch the Extension Development Host (`F5`) and open a `.ql` file with a
   type error (e.g. `examples/type_error.ql`) — a red squiggle should appear at
   the reported span, with the message in the Problems panel.
3. Fix the error and save — the squiggle clears.
4. Point `quilon.command` at a non-existent binary and reopen a `.ql` file — a
   single warning notification appears (it does not repeat).

To verify **`$` highlighting**, open `examples/unit.ql`: both the `-> $` return
type and the `$` value are colored like the built-in types (`Num`/`Text`/`Bool`).

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

- **Inline diagnostics (squiggles).** When you open or save a `.ql` file, the
  extension runs `<quilon.command> check <file>` in the background and surfaces
  any compile errors as editor squiggles in the Problems panel. The compiler
  reports errors as `path:line:col: error: <message>` with a caret underline;
  the extension parses those, converts the 1-based line/column to a VS Code
  range (using the caret run for the span width, falling back to the token at
  the column), and publishes them against the file. Diagnostics clear when the
  file checks clean. If the configured command can't be found, the extension
  warns **once** (set `quilon.command`, e.g. to `cargo run --`) and stays quiet
  thereafter.
- **No step debugging.** Real breakpoint/step debugging requires DWARF debug
  info, which the Quilon compiler does not emit yet — so this extension does
  **not** ship a debug adapter. The `.vscode/launch.json` contains only a
  trivial run-only configuration. A proper debugger is a future feature.

## Publishing

CI/CD for this extension lives in
[`.github/workflows/vscode-extension.yml`](../../.github/workflows/vscode-extension.yml):

- **PR gate (`validate`).** Every pull request and `main` push that touches
  `editors/vscode/**` validates the manifest/grammar/config JSON, type-checks
  and compiles the TypeScript (`npm run compile`), and runs
  `npx @vscode/vsce package` to prove the extension still builds into a `.vsix`.
- **Release (`publish`).** Pushing a tag matching `vscode-v*` packages the
  `.vsix`, attaches it to a GitHub Release for that tag, and — *if the
  maintainer secrets are set* — publishes to the VS Code Marketplace and
  Open VSX.

### Cutting a release

1. Bump `version` in [`package.json`](./package.json) — this is the version
   that gets published (vsce reads it from the manifest, not from the tag). Use
   a matching `vscode-v<version>` tag so the GitHub Release name lines up.
2. Tag and push, e.g. for version `0.1.0`:

   ```bash
   git tag vscode-v0.1.0
   git push origin vscode-v0.1.0
   ```

   The `publish` job builds the `.vsix` and creates the GitHub Release with the
   `.vsix` attached. This part needs **no secrets** — it always runs.

### Marketplace / Open VSX publishing (maintainer setup)

Publishing to the registries is **opt-in** and gated on repo secrets, so the
workflow succeeds for forks/contributors without credentials:

- **VS Code Marketplace** — set a `VSCE_PAT` repository secret (a
  [Personal Access Token](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#get-a-personal-access-token)
  for your Azure DevOps publisher). The `publisher` field in `package.json` is
  currently the placeholder `quilon`; replace it with your **real, registered**
  Marketplace publisher id before the first publish, since the PAT must belong
  to that publisher.
- **Open VSX** — set an `OVSX_PAT` repository secret
  ([Open VSX access token](https://github.com/eclipse/openvsx/wiki/Publishing-Extensions#3-create-an-access-token)).
  Before the first publish, create the namespace once (otherwise `ovsx publish`
  fails): `npx ovsx create-namespace quilon -p "$OVSX_PAT"` (use your real
  publisher id).

If a secret is absent the matching publish step is skipped and the run still
passes (release-only). Add either or both at
**Settings → Secrets and variables → Actions**.

> Note: `vsce package` warns that no `LICENSE` file is found inside the
> extension folder (the canonical license is `LICENSE.md` at the repo root).
> This is non-fatal. To surface a license on the Marketplace page, add a
> `LICENSE`/`LICENSE.md` under `editors/vscode/`.

## License

See [`LICENSE.md`](../../LICENSE.md) at the repo root.
