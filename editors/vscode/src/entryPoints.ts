// Detection of Quilon entry-point definitions (the top-level `^` function), kept
// free of any `vscode` import so it runs under plain Node and is unit-testable.
//
// In Quilon the program entry point is a top-level function named `^`, e.g.
//   ^ = () -> Num => < … >
//   ^ :: () -> Num
// The CodeLens provider uses this to place "▶ Run" / "Check" actions above each
// such definition.

/** A located entry-point definition: the 0-based line it starts on, and the
 *  0-based column of the `^` token (after any leading whitespace). */
export interface EntryPoint {
  /** 0-based line index of the definition. */
  line: number;
  /** 0-based column of the `^` token on that line. */
  column: number;
}

/**
 * A line is an entry-point definition when, after optional leading whitespace,
 * its first token is `^` followed by a binding/annotation (`=` or `::`) — i.e.
 * the entry-point forms `^ =` / `^ ::` (whitespace between `^` and the operator
 * is optional). Requiring `^` to be the *leading* token means a `^`-like
 * character inside a line comment (`~ … ^ …`) or a string (`"… ^ …"`) — where
 * the leading token is `~`/`x`/`"` instead — is never mistaken for a definition.
 */
const ENTRY_POINT = /^(\s*)\^\s*(?:=|::)/;

/**
 * Find all top-level `^` entry-point definitions in Quilon source text.
 *
 * Pure and `vscode`-free. Tolerant of leading whitespace and CRLF endings, and
 * deliberately conservative: it only matches when `^` is the first token on the
 * line, so a `^` inside a comment or string does not match.
 */
export function findEntryPoints(text: string): EntryPoint[] {
  const results: EntryPoint[] = [];
  const lines = text.split(/\r?\n/);
  for (let line = 0; line < lines.length; line++) {
    const match = ENTRY_POINT.exec(lines[line]);
    if (match) {
      results.push({ line, column: match[1].length });
    }
  }
  return results;
}
