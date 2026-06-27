// Pure parsing of the Quilon compiler's rustc-style diagnostic output.
//
// The compiler reports each error as a header line of the form
//
//   path/to/file.ql:LINE:COL: error: <message>
//
// optionally followed by a source-context block (a gutter line, the offending
// source line, and a caret underline) such as:
//
//   examples/type_error.ql:5:5: error: Unexpected token: TypeAnnotation
//     |
//   5 |   x :: Num = "not a number"
//     |     ^^
//
// LINE and COL are 1-based and count characters (not bytes). The caret run, when
// present, gives the width of the underlined span, which we use to size the
// diagnostic's range; otherwise it is a zero-width point.
//
// This module is deliberately free of any `vscode` imports so it can be unit
// tested with plain Node (`node:test`). The extension maps the results onto
// `vscode.Diagnostic` ranges.

/** A single parsed compiler diagnostic, with 1-based line/column as emitted. */
export interface ParsedDiagnostic {
  /** The path exactly as the compiler printed it (as passed on the CLI). */
  readonly file: string;
  /** 1-based line number. */
  readonly line: number;
  /** 1-based column number (character, not byte, offset). */
  readonly column: number;
  /** The error message text. */
  readonly message: string;
  /**
   * Number of characters the caret run underlined, if a source-context block
   * followed the header. `undefined` means no span was derivable (treat as a
   * zero-width point at the column).
   */
  readonly span?: number;
}

// A diagnostic header: "<path>:<line>:<col>: error: <message>".
//
// The path may itself contain ':' (e.g. Windows "C:\..."), so anchor on the
// final ":<digits>:<digits>: error:" suffix and treat everything before it as
// the path. `[^]` (any char incl. newline is irrelevant here since we match per
// line) keeps the path greedy up to the last line:col pair.
const HEADER_RE = /^(.*?):(\d+):(\d+): error: (.*)$/;

// A caret-underline line: optional gutter ("  | ") then one-or-more '^'.
const CARET_RE = /^\s*\|\s*(\^+)\s*$/;

/**
 * Parse all diagnostics out of the compiler's combined output (stderr/stdout).
 *
 * Lines that are not error headers (status banners, gutters, source echoes) are
 * ignored. When a header is immediately followed within the next few lines by a
 * caret line, its run length becomes the diagnostic's `span`.
 */
export function parseDiagnostics(output: string): ParsedDiagnostic[] {
  const lines = output.split(/\r?\n/);
  const diagnostics: ParsedDiagnostic[] = [];

  for (let i = 0; i < lines.length; i++) {
    const header = HEADER_RE.exec(lines[i]);
    if (!header) {
      continue;
    }

    const [, file, lineStr, colStr, message] = header;

    diagnostics.push({
      file,
      line: Number(lineStr),
      column: Number(colStr),
      message: message.trim(),
      span: findCaretSpan(lines, i),
    });
  }

  return diagnostics;
}

/**
 * Look just past a header line for the caret line of its source-context block,
 * returning the caret-run length. The block is "gutter | source / gutter |
 * carets", so the caret line is the second context line; scan a small window to
 * stay robust to formatting tweaks while not crossing into the next diagnostic.
 */
function findCaretSpan(lines: readonly string[], headerIndex: number): number | undefined {
  const WINDOW = 4;
  for (let i = headerIndex + 1; i <= headerIndex + WINDOW && i < lines.length; i++) {
    if (HEADER_RE.test(lines[i])) {
      return undefined; // Reached the next diagnostic without a caret line.
    }
    const caret = CARET_RE.exec(lines[i]);
    if (caret) {
      return caret[1].length;
    }
  }
  return undefined;
}
