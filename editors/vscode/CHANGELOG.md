# Changelog

All notable changes to the Quilon VS Code extension are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Quilon source is `.qn`; `.ql` is deprecated.** The language's extension changed (`.ql` is
  CodeQL's — see the compiler's changelog), so `.qn` is what the extension highlights, runs,
  checks, and debugs. `.ql` stays registered for the same transition the compiler gives it, so
  existing files keep working; the file icon is contributed per *language*, so it covers both.

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
