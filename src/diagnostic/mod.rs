//! Diagnostics as data, then as text.
//!
//! Every error the compiler reports is a [`Diagnostic`]: a [`Code`] from the registry, a
//! message in the language's own vocabulary, labelled byte spans, and an optional `help`.
//! The front end hands one to the CLI (which renders it) and to any other consumer — a
//! language server reads the spans directly.
//!
//! Rendering goes through `miette`'s graphical handler: colored on a terminal, plain
//! (no escapes, same frame) when stderr is redirected or `NO_COLOR` is set. A compile error
//! and a runtime report (`quilon-rt::report`) share this frame, so a failing assertion
//! reads like a type error:
//!
//! ```text
//! error[Q038]: no overload of `+` takes (Num, Bool)
//!    ╭─[program.qn:1:30]
//!  1 │ add = (a :: Num) -> Num => < a + true >
//!    ·                              ┬   ──┬─
//!    ·                              │     ╰── Bool
//!    ·                              ╰── Num
//!    ╰────
//!   help: the members of `+` are (Num, Num), (Text, Text)
//! ```

pub mod codes;

pub use codes::Code;

use crate::lexer::Span;
use crate::source_map::{SourceMap, shorten_path};
use miette::{GraphicalReportHandler, GraphicalTheme, LabeledSpan};
use unicode_bidi::{Level, ParagraphBidiInfo};

/// A span with what to say about it. The first label of a diagnostic is the one the
/// report opens at; the rest annotate the same file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    /// The text drawn under the span; `None` underlines it without a word.
    pub text: Option<String>,
}

/// One reported problem. Build it with [`Diagnostic::new`] and the builder methods, then
/// [`render`](Diagnostic::render) it against the sources it points into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub message: String,
    /// Empty for a problem with no place in the source (a file that cannot be read).
    pub labels: Vec<Label>,
    pub help: Option<String>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// A diagnostic with no location yet.
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            labels: Vec::new(),
            help: None,
            notes: Vec::new(),
        }
    }

    /// A located diagnostic whose span carries no words of its own.
    pub fn at(code: Code, span: &Span, message: impl Into<String>) -> Self {
        Self::new(code, message).label(span, None)
    }

    /// Add a span, with what to say under it.
    pub fn label(mut self, span: &Span, text: Option<String>) -> Self {
        self.labels.push(Label {
            span: span.clone(),
            text,
        });
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// The span the report opens at, if it has one.
    pub fn primary_span(&self) -> Option<&Span> {
        self.labels.first().map(|label| &label.span)
    }

    /// The report, drawn against `sources` — colored when `color` is set, plain otherwise.
    /// A label in a file `sources` does not know falls back to the root file, the way the
    /// old renderer did: naming the file being compiled beats naming nothing.
    pub fn render(&self, sources: &SourceMap, color: bool) -> String {
        let header = format!("error[{}]:", self.code);
        let mut theme = match color {
            true => GraphicalTheme::unicode(),
            false => GraphicalTheme::unicode_nocolor(),
        };
        theme.characters.error = header;
        let handler = GraphicalReportHandler::new_themed(theme)
            .with_context_lines(1)
            .with_wrap_lines(false);

        let mut rendered = String::new();
        let report = Report::new(self, sources);
        handler
            .render_report(&mut rendered, &report)
            .expect("rendering into a String cannot fail");
        // miette indents the message under a two-column icon; the header replaces the icon,
        // so the report opens at the margin like every other line of terminal output.
        let mut out = rendered
            .trim_end()
            .strip_prefix("  ")
            .unwrap_or(&rendered)
            .to_string();
        for note in &self.notes {
            out.push_str(&format!("\n  note: {note}"));
        }
        out
    }
}

/// One file as `miette` reads it. A snippet is the whole source line (or lines) a span
/// sits on and nothing more: the line before and after are not part of the report.
#[derive(Debug)]
struct Source {
    name: String,
    text: String,
}

impl miette::SourceCode for Source {
    /// `miette` asks twice: once with no context, for the position the frame opens at
    /// (`[file:line:column]`), and once with the handler's context for the lines to draw.
    /// The first answer is the span itself; the second is its whole line(s).
    fn read_span<'a>(
        &'a self,
        span: &miette::SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn miette::SpanContents<'a> + 'a>, miette::MietteError> {
        let text = &self.text;
        let start = span.offset().min(text.len());
        let end = (span.offset() + span.len()).clamp(start, text.len());
        let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
        let line = text[..line_start].matches('\n').count();
        let (from, to, column) = match context_lines_before + context_lines_after {
            0 => (start, end, text[line_start..start].chars().count()),
            _ => (
                line_start,
                text[end..].find('\n').map_or(text.len(), |at| end + at),
                0,
            ),
        };
        Ok(Box::new(miette::MietteSpanContents::new_named(
            self.name.clone(),
            &text.as_bytes()[from..to],
            (from, to - from).into(),
            line,
            column,
            text[from..to].matches('\n').count() + 1,
        )))
    }
}

/// [`Diagnostic`] as `miette` sees it: the message and labels bound to the one source they
/// point into.
#[derive(Debug)]
struct Report {
    message: String,
    help: Option<String>,
    source: Option<Source>,
    labels: Vec<LabeledSpan>,
}

impl Report {
    fn new(diagnostic: &Diagnostic, sources: &SourceMap) -> Self {
        let mut report = Self {
            message: diagnostic.message.clone(),
            help: diagnostic.help.clone(),
            source: None,
            labels: Vec::new(),
        };
        let Some(primary) = diagnostic.primary_span() else {
            return report;
        };
        let Some(located) = sources.locate_or_root(primary) else {
            return report;
        };
        let file = match sources.get_text(primary.file) {
            Some(_) => primary.file,
            None => crate::lexer::ROOT_FILE,
        };
        let text = sources.get_text(file).unwrap_or_default();
        report.source = Some(Source {
            name: shorten_path(&located.path),
            text: text.to_string(),
        });
        report.labels = diagnostic
            .labels
            .iter()
            .filter(|label| label.span.file == primary.file)
            .map(|label| {
                let (start, end) = visual_range(text, &label.span);
                LabeledSpan::new(label.text.clone(), start, end - start)
            })
            .collect();
        report
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Report {}

impl miette::Diagnostic for Report {
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help) as Box<dyn std::fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.source
            .as_ref()
            .map(|source| source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        match self.labels.is_empty() {
            true => None,
            false => Some(Box::new(self.labels.iter().cloned())),
        }
    }
}

/// The byte range whose characters a bidi-aware terminal DRAWS where `span`'s characters
/// sit, so the underline lands under what the span names. A span on a line with no
/// right-to-left run — the overwhelming common case — is returned untouched; a multi-line
/// span is clamped to its first line, since the underline only covers that one.
fn visual_range(source: &str, span: &Span) -> (usize, usize) {
    let start = (span.start as usize).min(source.len());
    let end = (span.end as usize).clamp(start, source.len());
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |at| start + at);
    let end = end.min(line_end);
    let line = &source[line_start..line_end];
    if line.is_ascii() {
        return (start, end.max(start));
    }
    let char_at = |byte: usize| line[..byte - line_start].chars().count();
    let (column, width) = (
        char_at(start) + 1,
        char_at(end).saturating_sub(char_at(start)),
    );
    let (visual_start, visual_width) = caret_position(line, column, width.max(1));
    let byte_of_char = |index: usize| {
        line.char_indices()
            .nth(index)
            .map_or(line.len(), |(byte, _)| byte)
            + line_start
    };
    (
        byte_of_char(visual_start),
        byte_of_char(visual_start + visual_width),
    )
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
    let last = visual_of_logical.len();
    let positions = (start..(start + width).min(last)).map(|i| visual_of_logical[i]);
    let (min, max) = positions.fold((usize::MAX, 0), |(min, max), pos| {
        (min.min(pos), max.max(pos))
    });
    match min == usize::MAX {
        true => (start, width),
        false => (min, max - min + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(text: &str) -> SourceMap {
        let mut map = SourceMap::default();
        map.set_root("f.qn", text);
        map
    }

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

    /// The underline over the first Hebrew letter is handed to the renderer as the byte
    /// range of the character drawn at that letter's visual column.
    #[test]
    fn a_span_on_a_bidi_line_is_moved_to_its_visual_bytes() {
        let text = "שלום world";
        let first_letter = Span::in_root(0, 2);
        let (start, end) = visual_range(text, &first_letter);
        // Visual column 3 is the fourth Hebrew letter, `ם`, at bytes 6..8.
        assert_eq!((start, end), (6, 8));
        assert_eq!(visual_range("abc def", &Span::in_root(4, 7)), (4, 7));
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
    fn a_plain_report_carries_the_code_position_source_line_and_underline() {
        let src = "add = 1 + true";
        let out = Diagnostic::at(Code::TypeMismatch, &Span::in_root(10, 14), "bad")
            .render(&sources(src), false);
        let expected = "\
error[Q028]: bad
   ╭─[f.qn:1:11]
 1 │ add = 1 + true
   ·           ────
   ╰────";
        assert_eq!(out, expected);
    }

    #[test]
    fn labels_help_and_notes_render_under_the_frame() {
        let src = "x = 1 + true";
        let out = Diagnostic::new(Code::NoMatchingOverload, "no `+` for these")
            .label(&Span::in_root(4, 5), Some("Num".to_string()))
            .label(&Span::in_root(8, 12), Some("Bool".to_string()))
            .help("interpolate instead")
            .note("a note")
            .render(&sources(src), false);
        assert!(out.starts_with("error[Q038]: no `+` for these\n"), "{out}");
        assert!(out.contains("─ Num"), "{out}");
        assert!(out.contains("─ Bool"), "{out}");
        assert!(out.contains("help: interpolate instead"), "{out}");
        assert!(out.ends_with("note: a note"), "{out}");
    }

    #[test]
    fn a_report_with_no_location_is_the_header_alone() {
        let out = Diagnostic::new(Code::SourceNotReadable, "gone").render(&sources(""), false);
        assert_eq!(out, "error[Q000]: gone");
    }

    #[test]
    fn a_colored_report_carries_escapes_and_a_plain_one_does_not() {
        let src = "x";
        let diagnostic = Diagnostic::at(Code::InvalidToken, &Span::in_root(0, 1), "m");
        assert!(diagnostic.render(&sources(src), true).contains("\x1b["));
        assert!(!diagnostic.render(&sources(src), false).contains("\x1b["));
    }

    #[test]
    fn render_shortens_a_path_too_long_for_the_position_line() {
        let path = format!("/{}/deep/module.qn", "d".repeat(80));
        let mut map = SourceMap::default();
        map.set_root(path, "x = 1");
        let out =
            Diagnostic::at(Code::InvalidToken, &Span::in_root(0, 1), "bad").render(&map, false);
        assert!(
            out.contains("╭─[…") && out.contains("/deep/module.qn:1:1]"),
            "a long path is shown from its end: {out}"
        );
    }
}
