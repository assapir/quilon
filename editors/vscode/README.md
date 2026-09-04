# Quilon for VS Code

Syntax highlighting and editor tasks for the [Quilon](../../README.md) programming
language — a statically-typed, **symbol-based** language (no keywords) that
compiles to native code via LLVM. Files use the `.qn` extension.

## Features

- **Syntax highlighting** for the full symbol set:
  - Comments (`~ to end of line`), strings (`"…"` with escapes), numbers, `true`/`false`, wildcard `_`.
  - The entry point `^`, module import `<<` (with the imported path), and export marker `>>`.
  - Operators: `:=` (mutable bind) vs `=` (immutable bind), `::` (type annotation),
    `=>` (function body / match arm), `->` (return type), `<-` (inclusive range),
    `?` / `|` (pattern matching), arithmetic `+ - * / %`, comparison `== != < <= > >=`,
    logical `&& || !`. Each **multi-character** operator (`=>`, `->`, `:=`,
    `<-`, `::`, `==`, `!=`, `<=`, `>=`, `&&`, `||`) is highlighted as a **single**
    token — never split into its first character colored separately from the rest.
  - **`< >` block delimiters** — a line-final `<` (opens a block) and a `>` with no
    operand after it on its line (the block close) share one block-punctuation
    scope, so both delimiters color identically; a `<`/`>` with an operand after it
    stays the less-than / greater-than comparison operator.
  - Built-in types `Num` / `Text` / `Bool`, and the unit type/value `$` (`$` is
    both the type, as in `-> $`, and its sole value — highlighted like the other
    built-in types).
  - **Capitalized identifiers** are highlighted as types / sum-type constructors
    (`Ok`, `NotOk`, `Color`, `Circle`); **lowercase** names followed by `(` as function calls.
- **Bracket matching & auto-closing** for `< >`, `{ }`, `[ ]`, `( )`, and `"`.
- **Language server integration** — the extension spawns `quilon lsp` (the
  compiler's own language server) and receives from it (see
  [The language server](#the-language-server)):
  - **Inline diagnostics** — type/parse/lex errors as editor squiggles, live
    against the buffer as you type (no save needed).
  - **Go to definition** — on functions, variables, parameters, and names an
    import supplies (jumping into the imported file).
  - **Hover** — the inferred type of the expression under the cursor.
  - **Semantic tokens** — block `< >` delimiters colored apart from the `<` / `>`
    comparison operators, plus declared type, function, and parameter names.
  - **Test CodeLens** — **▶ Run suite** / **🐞 Debug suite** and **▶ Run case** /
    **🐞 Debug case** actions above each `describe` and `it` block: Run executes that
    suite or case, Debug launches it under CodeLLDB with breakpoints honoured.
- **Test Explorer** — the "Testing" view lists every `describe`/`it` in an open `.qn`
  file, built from the language server's `quilon/testItems`; a ▶ Run there runs the
  selection through `quilon test --reporter json` and reports pass/fail per case, and a
  🐞 Debug profile launches each selected item under CodeLLDB (see
  [Test Explorer](#test-explorer)).
- **Editor tasks & commands** to run the compiler on the active file.
- **CodeLens** — **▶ Run** and **▶ Debug** actions appear above each `^`
  entry-point definition (see [Running the compiler](#running-the-compiler-from-the-editor)).
- **Debugging** — set breakpoints in `.qn` source and step through a native
  build under [CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb)
  (see [Debugging](#debugging)).

## Install / run locally

The extension is written in **TypeScript** (`src/extension.ts`), compiled to
`out/extension.js` by `tsc`. It uses [**pnpm**](https://pnpm.io) as its package
manager (the pinned version lives in the `packageManager` field of
`package.json`; `corepack enable` will provision it automatically). Install
dependencies once before building or debugging:

```bash
cd editors/vscode
pnpm install
```

This extension is not published to the Marketplace. To try it from this checkout:

### Option A — Extension Development Host (recommended)

1. Open the `editors/vscode/` folder in VS Code.
2. Press `F5` ("Run Extension"). The `compile` preLaunchTask builds the
   TypeScript, then a new "Extension Development Host" window opens.
3. Open any `.qn` file (e.g. one from `examples/`) — highlighting is active.

Use `pnpm run watch` for incremental recompiles while iterating.

### Option B — install as a `.vsix`

```bash
cd editors/vscode
pnpm install                  # if you haven't already
pnpm run package              # compiles + produces quilon-0.1.0.vsix (via vsce)
code --install-extension quilon-0.1.0.vsix
```

## Development

The extension is TypeScript (strict). It is linted with **oxlint** and formatted
with **oxfmt** (the [Oxc](https://oxc.rs) toolchain) — not ESLint/Prettier:

```bash
pnpm run compile    # tsc type-checks src/, then rolldown bundles src/extension.ts -> out/extension.js
pnpm test           # compile, then run the unit tests (node --test)
pnpm run lint       # oxlint (fails on any finding)
pnpm run lint:fix   # oxlint --fix (auto-fix what it can)
pnpm run fmt        # oxfmt --write (format in place)
pnpm run fmt:check  # oxfmt --check (verify formatting; CI gate)
```

`pnpm run compile` type-checks every file under `src/` with `tsc` (also emitting the
per-file `out/*.js` the unit tests run against), then bundles the extension's
entry point with [rolldown](https://rolldown.rs) — `rolldown.config.mjs` — into a
single `out/extension.js`, `vscode` left external since the extension host
provides it. `pnpm run watch` keeps using plain `tsc -watch` for fast
iteration in the Extension Development Host; run `pnpm run compile` before
packaging to refresh the bundle.

CI runs `lint`, `fmt:check`, `compile`, `test`, and `package` on every PR that
touches `editors/vscode/**` (see [Publishing](#publishing)).

### Tests & manual verification

Unit tests (`pnpm test`) cover the extension's pure logic, all kept free of any
`vscode` import so they run under plain Node:

- compiler resolution (`src/compilerCommand.ts` ↔ `src/compilerCommand.test.ts`) —
  the search order above, over an injected view of a make-believe machine
  (`PATH`, install directories, open folders, `PATHEXT` on Windows), and the
  `quilon lsp` invocation the language client spawns;
- the entry-point detector behind the CodeLens (`src/entryPoints.ts` ↔
  `src/entryPoints.test.ts`);
- the debug build/launch helpers (`src/debugConfig.ts` ↔ `src/debugConfig.test.ts`) —
  the `build --debug` and `test --binary` argv, the resolved CodeLLDB configuration, and
  the in-flight-build guard;
- the Test Explorer's tree-building and NDJSON parsing (`src/testRunner.ts` ↔
  `src/testRunner.test.ts`) — nesting a flat `quilon/testItems` list by its `/`-joined
  paths, the `quilon test --reporter json --only …` argv, and parsing `--reporter json`
  events (including malformed/truncated lines, which must not throw);
- **grammar tokenization** (`src/grammar.test.ts`) — it loads the real
  `syntaxes/quilon.tmLanguage.json` and asserts each multi-character operator
  (`=>`, `->`, `:=`, `<-`, `::`, `==`, `!=`, `<=`, `>=`, `&&`, `||`)
  tokenizes to a **single** scope, plus regression guards for `<` / `>`, `=`,
  `$`, comments, strings, and numbers. `src/grammar.ts` is a tiny dependency-free
  re-implementation of TextMate's ordered first-match-wins rule (the behaviour
  the operator ordering relies on), so no native engine is needed.

To verify the **language server** end-to-end manually:

1. Have a working compiler — installed (`cargo install --path .`), built in the
   open checkout, or named by `quilon.command` (e.g. `"cargo run --"`).
2. Launch the Extension Development Host (`F5`) and open a `.qn` file with a
   type error (e.g. `examples/type_error.qn`) — a red squiggle appears at the
   reported span, with the message in the Problems panel.
3. Fix the error — the squiggle clears as you type, no save needed.
4. Hover an expression (its inferred type appears), Ctrl/Cmd-click a name (its
   definition opens), and open a test file (Run and Debug lenses appear above each
   `describe` and `it`) — click **🐞 Debug case** on one and confirm a CodeLLDB session
   starts, with a breakpoint set inside the case honoured.
5. Open the "Testing" view — the file's suites and cases appear, nested; run one
   and watch it turn green or red as `quilon test --reporter json` reports it, then
   debug one and confirm the same CodeLLDB session starts from there.
6. Point `quilon.command` at a non-existent binary and reload — a warning
   notification appears, naming that setting and offering **Open Settings**.

To verify **`$` highlighting**, open `examples/unit.qn`: both the `-> $` return
type and the `$` value are colored like the built-in types (`Num`/`Text`/`Bool`).

## Running the compiler from the editor

Two commands are contributed (open the Command Palette, `Ctrl/Cmd+Shift+P`):

- **Quilon: Check Current File** → runs `quilon check <file>`
- **Quilon: Run Current File** → runs `quilon run <file>`

They run in an integrated terminal named "Quilon".

### Which compiler gets run

Left at its default, `quilon.command` is not a literal command — the extension
locates a compiler itself, taking the first of:

1. `quilon` on the `PATH` the editor's process inherited;
2. `quilon` in a usual install directory — `~/.cargo/bin`, `~/.local/bin`,
   `/usr/local/bin`, `/opt/homebrew/bin`;
3. a `target/release/quilon` or `target/debug/quilon` built in an open folder;
4. `cargo run --quiet --`, when an open folder is a checkout of the compiler repo
   (its `Cargo.toml` names the crate) and `cargo` is available.

Step 2 is what makes an editor started from a desktop launcher work: a GUI
process inherits no `PATH` from your shell rc, so a `cargo install`ed compiler is
on none of it.

To pin the invocation instead, set the setting — it is then used verbatim, with
no search (a bare `"quilon"` is the exception: it says nothing the default
doesn't, so it searches too):

```jsonc
// settings.json
"quilon.command": "cargo run --"
```

Every feature that runs the compiler — the language server, Check, Run, the
test lenses, and the debug build — uses the one resolution, so they never
disagree about which compiler this workspace has. If none can be spawned, the
notification says where it looked and offers to open the setting.

The bundled `.vscode/tasks.json` also provides **quilon: check current file**
and **quilon: run current file** tasks (`Terminal → Run Task…`).

### CodeLens above the entry point

Every executable Quilon program defines a top-level `^` entry point (its
`main`). Above each `^` definition the extension shows two clickable CodeLens
actions:

- **▶ Run** — invokes **Quilon: Run Current File** (`quilon run <file>`).
- **▶ Debug** — builds the file with `quilon build --debug` and launches it
  under CodeLLDB, so breakpoints set in the `.qn` source are hit (see
  [Debugging](#debugging)).

Both act on the file containing the lens. Test files get their own lenses —
**▶ Run suite** / **🐞 Debug suite** and **▶ Run case** / **🐞 Debug case** above each
`describe` and `it` — from [the language server](#the-language-server).

## The language server

On activation the extension spawns the compiler's own language server —
`<quilon.command> lsp` — and connects to it over stdio with
[`vscode-languageclient`](https://www.npmjs.com/package/vscode-languageclient).
The server runs the real compiler front end (lex → parse → resolve imports →
type-check) over the editor's buffer on every change, so everything it reports
is the compiler's own verdict on the text as it stands — unsaved edits
included. It provides:

- **Diagnostics** — published on open and on every change; they clear when the
  buffer checks clean. In a test file, the test bodies are checked too (they
  are what `quilon test` runs).
- **Go to definition** — resolves through the compiler's scopes: parameters,
  block locals, pattern bindings, top-level functions, and names an `<<` import
  supplies (the jump lands in the imported file).
- **Hover** — the inferred type of the smallest expression under the cursor,
  from the type checker's own table.
- **Semantic tokens** — block `< >` delimiters versus `<` / `>` comparisons,
  plus declared type / function / parameter names.
- **Test CodeLens** — a **▶ Run suite** / **▶ Run case** and a **🐞 Debug suite** /
  **🐞 Debug case** lens above every `describe` and `it`, both carrying the block's own
  `/`-joined path. Run invokes the extension's `quilon.runTests` command, which runs
  `quilon test <file> --only <path>` in the "Quilon" terminal — a suite lens runs that
  suite, a case lens that case. Debug invokes `quilon.debugTests`, which builds
  `quilon test <file> --only <path> --binary <tmp>` and launches the result under
  CodeLLDB, so breakpoints in the case are hit (see [Debugging](#debugging)).
- **`quilon/testItems`** — the same test tree the lenses read, as one flat list (each
  entry's path, name, kind, and range), which the Test Explorer builds its tree from
  instead of re-parsing the file itself.

If the server cannot be spawned, a warning notification names the
`quilon.command` setting and offers **Open Settings**.

See [the language server's reference page](../../docs/tooling/language-server.md)
for the protocol surface and for wiring other editors.

## Test Explorer

The "Testing" view (the flask icon in the Activity Bar) lists every `describe`/`it`
found in an open `.qn` file, one node per suite and case, nested the way they're
written. The tree is built from `quilon/testItems` — the same data the CodeLens read —
and refreshes when a `.qn` document opens, is saved, or is edited (debounced).

Selecting **▶ Run** on a node runs `quilon test <file> --reporter json`, adding
`--only <path>` for each selected suite or case (a whole-file run when the file's own
root node is selected); several selected files run in parallel, each its own process.
The run's NDJSON events are parsed back into the view: each case turns green or red as
its result arrives, and a failing case's message and `file:line` appear inline —
Ctrl/Cmd-click the location to jump there, the same as any other test failure VS Code
reports. **Run All** (▶ at the top of the view) runs every known file's suites.

Selecting **🐞 Debug** on a node builds that item — `quilon test <file> [--only <path>]
--binary <tmp>` (whole file when the file's own root node is selected) — and launches the
result under CodeLLDB, breakpoints in the case honoured. Several selected items debug
one after another. A debugged item is marked started; its outcome is whatever you observe
while stepping through it.

## Debugging

Source-level debugging is delegated to [**CodeLLDB**](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb),
declared as an extension dependency so VS Code installs it alongside Quilon.

The `quilon` debug type does two things when a session starts:

1. Builds the active `.qn` with `<quilon.command> build --debug <file> -o <tmp>`,
   which emits DWARF line info into the native binary.
2. Launches that binary under CodeLLDB (`type: "lldb"`).

Breakpoints you set in the source and single-stepping both work. Start a session with the
**▶ Debug** CodeLens above `^`, the **Quilon: Debug Current File** command, or a
`launch.json` entry:

```jsonc
{
  "type": "quilon",
  "request": "launch",
  "name": "Quilon: Debug current file",
  "program": "${file}",
  "args": []
}
```

**Value inspection.** The lldb formatter the session loads
(`formatters/quilon.py`) renders Quilon values against the distinct DWARF types
the compiler emits: a `Text` shows as its string (not a `{data, byte_len}`
struct), and a `[]T` expands to an indexed list of its elements, each keeping its
own type — so a `[][]Text` expands to a list of inner `[]Text` arrays, each of
its own `Text` values. Long arrays cap the default expansion and note the
remaining count in the summary (an explicit `array[i]` past the cap still works).
Records and sum types fall back to lldb's default struct rendering.

## Publishing

CI/CD for this extension lives in
[`.github/workflows/vscode-extension.yml`](../../.github/workflows/vscode-extension.yml):

- **PR gate (`validate`).** Every pull request and `main` push that touches
  `editors/vscode/**` validates the manifest/grammar/config JSON, type-checks
  and bundles the TypeScript (`pnpm run compile`), and runs
  `pnpm exec vsce package` to prove the extension still builds into a `.vsix` —
  then asserts the `.vsix` stays under 40 files, keeping the bundle from
  regressing back to shipping `node_modules` unbundled.
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
  the placeholder `quilon`; replace it with your **real, registered**
  Marketplace publisher id before the first publish, since the PAT must belong
  to that publisher.
- **Open VSX** — set an `OVSX_PAT` repository secret
  ([Open VSX access token](https://github.com/eclipse/openvsx/wiki/Publishing-Extensions#3-create-an-access-token)).
  Before the first publish, create the namespace once (otherwise `ovsx publish`
  fails): `pnpm dlx ovsx create-namespace quilon -p "$OVSX_PAT"` (use your real
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
