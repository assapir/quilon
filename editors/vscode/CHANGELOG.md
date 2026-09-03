# Changelog

All notable changes to the Quilon VS Code extension are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0] - 2026-09-03

Version matches the Quilon compiler it targets.

### Added

- **A real language server.** The extension now spawns `quilon lsp` (via
  `vscode-languageclient`) instead of shelling out to `quilon check` for diagnostics.
  Diagnostics, go-to-definition, hover, and semantic tokens (`<`/`>` correctly classified as
  block punctuation or the comparison operator) all come from the compiler's own front end
  now, over an open buffer's live text rather than what was last saved.
- **Test Explorer.** A `vscode.tests.createTestController` view, filled from the language
  server's `quilon/testItems` request and refreshed on open/save/change. Its Run profile
  spawns `quilon test <file> --reporter json [--only <path> ...]` per selected file and
  reports pass/fail with file:line as results arrive. There is no Debug profile for tests —
  `quilon test` runs only under the in-process JIT, and the extension's only debug path
  launches a native `--debug` build under CodeLLDB.
- **CodeLenses per test.** A **▶ Run suite** lens above every `describe` and a **▶ Run case**
  lens above every `it`, each running just that suite or case rather than the whole file.

### Changed

- **Breaking: the pipe operator `|>` is gone from the grammar.** It no longer highlights —
  matching its removal from the compiler.

### Fixed

- **A debug session no longer fails with `could not run "quilon"` when the compiler is
  installed.** The extension ran a bare `quilon`, which only resolves if the editor's process
  inherited a `PATH` containing it — an editor started from a desktop launcher typically has
  not, since `~/.cargo/bin` is added by a shell rc file. With `quilon.command` at its default,
  the extension now locates the compiler itself: `PATH`, the usual install directories
  (`~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`, `/opt/homebrew/bin`), a
  `target/release`|`target/debug` build in an open folder, and finally `cargo run --quiet --`
  when an open folder is a checkout of the compiler repo. A `quilon.command` you set is still
  used verbatim — except a bare `"quilon"`, which is exactly the value that leaves a
  GUI-launched editor with nothing to run, and so searches too. Diagnostics, Run, Check, and Debug all share the one resolution, and when no
  compiler can be spawned the notification says where it looked and offers to open the
  setting.

## [0.9.2] - 2026-08-24

Version matches the Quilon compiler it targets.

### Changed

- **Breaking: Quilon source is `.qn`**, and `.ql` is not a Quilon file any more — it no
  longer highlights, and Run / Check / Debug do not act on it. Rename any `.ql` source you
  still have; the compiler no longer accepts it either. (`.ql` is CodeQL's extension — see
  the compiler's changelog.)

### Added

- **Debugging** via [CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb)
  (declared as an extension dependency). A `quilon` debug type builds the active
  `.qn` with `quilon build --debug` and launches the native binary under
  CodeLLDB, so breakpoints set in the `.qn` source and single-stepping resolve
  against the source through the compiler's DWARF line table.
- **▶ Debug CodeLens** above every `^` entry point, next to **▶ Run**, plus a
  **Quilon: Debug Current File** command and a contributed default `launch.json`
  configuration.
- **lldb value formatters** in `formatters/quilon.py`, imported into the debug
  session. A `Text` renders as its string and a `[]T` expands to an indexed list
  of its elements (each keeping its own type, so `[][]Text` nests), with a
  size-aware summary and a cap on element expansion. Records and sum types fall
  back to lldb's default rendering for now.
- **Debug build progress** — starting a debug session now shows a
  "building … for debug" notification while `quilon build --debug` runs, so the
  otherwise-silent build gives visible feedback. A second ▶ Debug for the same
  file is refused while its build is still in flight, so an impatient re-click
  can't kick off a duplicate build.
- **Debug output reuse** — debug sessions send the program's output to the
  shared Debug Console instead of spawning a fresh integrated terminal each run,
  so terminals no longer accumulate across sessions.

## [0.9.1] - 2026-08-10

Initial release. Version matches the Quilon compiler it targets.

### Added

- **Syntax highlighting** for Quilon (`.ql`) source files, driven by a
  TextMate grammar covering the language's symbol-based syntax (entry points,
  pattern matching, pipelines, comments, records, and sum types). Every
  multi-character operator (`->`, `=>`, `<-`, `|>`, `::`, `:=`, `==`, `!=`,
  `<=`, `>=`, `&&`, `||`) is a single token with a single scope, so it renders
  in one color and never splits mid-glyph. A line-final `<`/`>` is scoped as
  block punctuation on a safe best-effort basis; fully correct block `< >`
  coloring needs semantic tokens / an LSP and is deferred.
- **Inline diagnostics** — on open and on save of a `.ql` file, the extension
  runs `quilon check` on it, parses the compiler's `path:line:col: error:`
  output, and surfaces each error as an in-editor squiggle. Diagnostics update
  as you save and are cleared when a file checks clean.
- **Run CodeLens** — a "▶ Run" action appears above every top-level `^`
  entry-point definition, invoking the compiler on the current file in an
  integrated terminal.
- **Commands** — "Quilon: Check Current File" and "Quilon: Run Current File",
  available from the Command Palette.
- **Configurable compiler invocation** via the `quilon.command` setting
  (defaults to `quilon` on your `PATH`; set it to e.g. `cargo run --` to drive
  the compiler from a checkout).
- **File icon** for `.ql` files, contributed via the language `icon`
  contribution (light and dark variants), shown by icon themes that defer to it.

### Fixed

- **Operator coloring** — every multi-character operator (`->`, `=>`, `<-`,
  `|>`, `::`, `:=`, `==`, `!=`, `<=`, `>=`, `&&`, `||`) now tokenizes as a
  single token with a single scope, so it renders in one color and no longer
  splits mid-glyph (which also broke ligatures). The grammar lists every
  multi-char operator rule before the single-char rules it shares a prefix with,
  since TextMate is first-match-wins at each position. A line-final `<`/`>` is
  scoped as block punctuation on a safe best-effort basis; fully correct block
  `< >` coloring needs semantic tokens / an LSP and is deferred.

### Changed

- Simplified the entry-point CodeLens to Run only; type-checking remains
  available through on-save inline diagnostics and the "Quilon: Check Current
  File" command.
- Made the `.ql` file icon font-independent by shipping pre-rendered PNGs, so
  the glyph renders correctly without the previously-embedded font installed.
