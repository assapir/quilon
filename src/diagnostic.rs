//! rustc-style diagnostic rendering.
//!
//! Compiler errors carry a [`Span`] of byte offsets into the source. On its own
//! that is unreadable (`Span { start: 42, end: 47 }`); this module turns a span
//! plus the original source text into a human- and tooling-friendly report:
//!
//! ```text
//! path/to/file.qn:3:9:
//! error: Type mismatch: expected Num, got Bool
//!   |
//! 3 |     x = 1 + true
//!   |         ^^^^^^^^
//! ```

use crate::lexer::Span;
use crate::source_map::{locate_in, shorten_path};
use unicode_bidi::{Level, ParagraphBidiInfo};

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
/// is its full text. `span` is the byte range the message refers to. The position and
/// caret width come from [`locate_in`] — the same resolver a runtime `Site` uses — so a
/// compile error and a failing assertion point at a span identically (for a multi-line
/// span, only the first line is underlined).
pub fn render(path: &str, source: &str, span: &Span, severity: Severity, message: &str) -> String {
    let loc = locate_in(path, source, span);
    // Position first, then the message on its OWN line: the two are different questions
    // ("where?" and "what?"), and a long message no longer pushes the position off the
    // right edge. The path is shortened from the start if it would not fit.
    let mut out = format!(
        "{}:{}:{}:\n{}: {message}",
        shorten_path(path),
        loc.line,
        loc.column,
        severity.label()
    );

    // A span past the end of the file has no line to show; the position alone is the
    // whole diagnostic.
    let Some(excerpt) = &loc.excerpt else {
        return out;
    };

    // Width of the line-number gutter, e.g. "3 | " -> the "3" plus a space.
    let line_no = loc.line.to_string();
    let gutter = " ".repeat(line_no.len());

    out.push_str(&format!("\n{gutter} |"));
    // The source line itself is echoed exactly as written — logical order, untouched (see
    // docs/types/text.md: no operation ever reorders `Text` data or output). Only the
    // caret's own position accounts for how a bidi-aware terminal will DISPLAY that line.
    out.push_str(&format!("\n{line_no} | {excerpt}"));
    let (caret_start, caret_width) = caret_position(excerpt, loc.column, loc.width);
    out.push_str(&format!(
        "\n{gutter} | {}{}",
        " ".repeat(caret_start),
        "^".repeat(caret_width)
    ));
    out
}

/// The caret underline's 0-based start column and character width within `excerpt`, given
/// the span's LOGICAL column (`column`, 1-based) and character width (`width`).
///
/// On a plain ASCII line — the overwhelming common case — visual order is logical order by
/// definition, so this returns the logical position untouched without touching the bidi
/// crate at all. A line with right-to-left characters instead resolves the line's VISUAL
/// order per UAX #9, with an LTR paragraph level (a diagnostic line begins with its
/// gutter), so the underline lands under the grapheme(s) the span names wherever a
/// bidi-aware terminal actually draws them — not wherever they sit in the untouched byte
/// order printed above.
fn caret_position(excerpt: &str, column: usize, width: usize) -> (usize, usize) {
    let start = column - 1; // 0-based logical char index

    if excerpt.is_ascii() {
        return (start, width);
    }

    let paragraph = ParagraphBidiInfo::new(excerpt, Some(Level::ltr()));
    if paragraph.is_pure_ltr {
        return (start, width); // no RTL run resolved: visual order equals logical order
    }

    // One level per character (not byte), aligned with `excerpt.chars()` — the same units
    // `column`/`width` are already counted in.
    let levels = paragraph.reordered_levels_per_char(0..excerpt.len());
    // `visual[v] == logical char index shown at visual position v` (Rule L2). Inverted, it
    // answers the question the caret needs: where does logical char `i` land visually?
    let visual = ParagraphBidiInfo::reorder_visual(&levels);
    let mut visual_of_logical = vec![0usize; visual.len()];
    for (visual_pos, &logical_char) in visual.iter().enumerate() {
        visual_of_logical[logical_char] = visual_pos;
    }

    // The span's characters may not land on contiguous visual columns (e.g. a span
    // straddling an LTR/RTL boundary), so the caret covers their bounding range — the same
    // simplification a terminal's own selection highlighting makes.
    let positions = (start..start + width).map(|i| visual_of_logical[i]);
    let (min, max) = positions.fold((usize::MAX, 0), |(min, max), pos| {
        (min.min(pos), max.max(pos))
    });
    (min, max - min + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Hebrew run inside an otherwise-LTR line reorders for display: UAX #9 mirrors an
    /// odd-level (RTL) run's characters, so the FIRST character typed — `ש`, the rightmost
    /// letter a reader sees first — is drawn LAST among the run's four characters. The
    /// caret over just that one character must therefore land on the visual column the
    /// mirrored run actually draws it at, not on its untouched logical column (0).
    #[test]
    fn caret_over_a_hebrew_character_lands_on_its_visual_column() {
        let excerpt = "שלום world"; // "שלום" (4 Hebrew letters) + " world" (6 ASCII chars)
        let (caret_start, caret_width) =
            caret_position(excerpt, /* column */ 1, /* width */ 1);

        // Computed independently, straight off the crate's own reordering primitives — the
        // ground truth for where a bidi-aware terminal actually draws this character.
        let paragraph = ParagraphBidiInfo::new(excerpt, Some(Level::ltr()));
        assert!(
            !paragraph.is_pure_ltr,
            "the line must contain a real RTL run"
        );
        let levels = paragraph.reordered_levels_per_char(0..excerpt.len());
        let visual = ParagraphBidiInfo::reorder_visual(&levels);
        let expected_start = visual
            .iter()
            .position(|&logical_char| logical_char == 0)
            .unwrap();

        assert_eq!((caret_start, caret_width), (expected_start, 1));
        // A 4-letter RTL run reversed for display puts its first logical character last —
        // visual column 3, not the untouched logical column 0. Pinning the actual number
        // (not just "differs from 0") is what catches a reordering bug that still moves it,
        // just to the wrong place.
        assert_eq!(expected_start, 3);
    }

    /// The fast path: a pure-ASCII line never touches the bidi crate, and produces exactly
    /// the untouched logical column/width — the existing (pre-bidi-awareness) behavior.
    #[test]
    fn caret_on_an_ascii_line_is_the_untouched_logical_position() {
        assert_eq!(caret_position("add = 1 + true", 11, 4), (10, 4));
    }

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
        let out = render("f.qn", src, &Span::in_root(10, 14), Severity::Error, "bad");
        let expected = "\
f.qn:1:11:
error: bad
  |
1 | add = 1 + true
  |           ^^^^";
        assert_eq!(out, expected);
    }

    #[test]
    fn render_uses_the_spans_own_line() {
        let src = "line one\nx = oops\nline three";
        let out = render("f.qn", src, &Span::in_root(13, 17), Severity::Error, "boom");
        assert!(out.contains("f.qn:2:5:\nerror: boom"), "{out}");
        assert!(out.contains("2 | x = oops"), "{out}");
        assert!(out.contains("    ^^^^"), "{out}");
    }

    #[test]
    fn render_clamps_multiline_span_to_first_line() {
        // A span that runs off the end of its line only underlines line one.
        let src = "abc\ndef";
        let out = render("f.qn", src, &Span::in_root(0, 7), Severity::Error, "x");
        // 3 carets under "abc", not 7.
        assert!(out.ends_with("| ^^^"), "{out}");
    }

    #[test]
    fn render_shortens_a_path_too_long_for_the_position_line() {
        let path = format!("/{}/deep/module.qn", "d".repeat(80));
        let out = render(&path, "x = 1", &Span::in_root(0, 1), Severity::Error, "bad");
        let position = out.lines().next().unwrap_or_default();
        assert!(
            position.starts_with('…') && position.ends_with("/deep/module.qn:1:1:"),
            "a long path is shown from its end: {position}"
        );
    }
}
