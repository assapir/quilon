//! rustc-style diagnostic rendering.
//!
//! Compiler errors carry a [`Span`] of byte offsets into the source. On its own
//! that is unreadable (`Span { start: 42, end: 47 }`); this module turns a span
//! plus the original source text into a human- and tooling-friendly report:
//!
//! ```text
//! path/to/file.ql:3:9: error: Type mismatch: expected Num, got Bool
//!   |
//! 3 |     x = 1 + true
//!   |         ^^^^^^^^
//! ```

use crate::lexer::Span;

/// How a diagnostic is labelled (`error`, `warning`, ...). Quilon only emits
/// errors today, but the renderer is severity-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
        }
    }
}

/// Render a single diagnostic in the `path:line:col: error: message` style with
/// the offending source line and a caret underline beneath the span.
///
/// `path` is the source file as the user named it on the command line. `source`
/// is its full text. `span` is the byte range the message refers to; for a
/// multi-line span only the first line is underlined.
pub fn render(path: &str, source: &str, span: &Span, severity: Severity, message: &str) -> String {
    let (line, col) = Span::line_col(source, span.start);
    let mut out = format!("{path}:{line}:{col}: {}: {message}", severity.label());

    // The text of the line the span starts on (without its trailing newline).
    let Some(line_text) = source.lines().nth(line - 1) else {
        return out;
    };

    // Width of the line-number gutter, e.g. "3 | " -> the "3" plus a space.
    let line_no = line.to_string();
    let gutter = " ".repeat(line_no.len());

    // Caret run: `col - 1` chars of lead, then underline the span's width,
    // clamped to what is left on this line (multi-line spans stay on line one)
    // and to at least one caret so an empty/zero-width span is still pointed at.
    let lead = col - 1;
    let span_chars = char_len(source, span.start, span.end);
    let remaining = line_text.chars().count().saturating_sub(lead);
    let underline = span_chars.clamp(1, remaining.max(1));

    out.push_str(&format!("\n{gutter} |"));
    out.push_str(&format!("\n{line_no} | {line_text}"));
    out.push_str(&format!(
        "\n{gutter} | {}{}",
        " ".repeat(lead),
        "^".repeat(underline)
    ));
    out
}

/// Number of `char`s in `source[start..end]`, clamped to the source bounds.
/// Used for the caret width so it counts scalar values, not bytes.
fn char_len(source: &str, start: usize, end: usize) -> usize {
    let start = start.min(source.len());
    let end = end.min(source.len()).max(start);
    source[start..end].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based() {
        let src = "ab\ncde\nf";
        assert_eq!(Span::line_col(src, 0), (1, 1)); // 'a'
        assert_eq!(Span::line_col(src, 1), (1, 2)); // 'b'
        assert_eq!(Span::line_col(src, 3), (2, 1)); // 'c' (after first '\n')
        assert_eq!(Span::line_col(src, 5), (2, 3)); // 'e'
        assert_eq!(Span::line_col(src, 7), (3, 1)); // 'f'
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        // 'é' is two bytes; the 'x' after it is byte offset 3 but column 3.
        let src = "aéx";
        assert_eq!(Span::line_col(src, 3), (1, 3));
    }

    #[test]
    fn line_col_clamps_past_end() {
        let src = "ab";
        assert_eq!(Span::line_col(src, 99), (1, 3));
    }

    #[test]
    fn render_points_at_the_span() {
        let src = "add = 1 + true";
        // Underline "true" (bytes 10..14).
        let out = render("f.ql", src, &Span::new(10, 14), Severity::Error, "bad");
        let expected = "\
f.ql:1:11: error: bad
  |
1 | add = 1 + true
  |           ^^^^";
        assert_eq!(out, expected);
    }

    #[test]
    fn render_uses_the_spans_own_line() {
        let src = "line one\nx = oops\nline three";
        let out = render("f.ql", src, &Span::new(13, 17), Severity::Error, "boom");
        assert!(out.contains("f.ql:2:5: error: boom"), "{out}");
        assert!(out.contains("2 | x = oops"), "{out}");
        assert!(out.contains("    ^^^^"), "{out}");
    }

    #[test]
    fn render_clamps_multiline_span_to_first_line() {
        // A span that runs off the end of its line only underlines line one.
        let src = "abc\ndef";
        let out = render("f.ql", src, &Span::new(0, 7), Severity::Error, "x");
        // 3 carets under "abc", not 7.
        assert!(out.ends_with("| ^^^"), "{out}");
    }
}
