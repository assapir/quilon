//! Where a span points: the path and text of every file that went into one compilation.
//!
//! Byte offsets restart at 0 in every module, so a [`Span`] carries a [`FileId`] to stay
//! unique across the merged program. This map is the other half of that identity — it
//! turns a `FileId` back into the file's display path and source text, which is what a
//! human-readable position needs.
//!
//! Two things consume it: compiler diagnostics (`crate::diagnostic`), and the `Site`
//! values codegen materializes at a call site (see `CodeGenerator::site_value`). Both
//! resolve a span through [`locate`](SourceMap::locate) / [`locate_in`], so a compile error
//! and a failing assertion report the same line, column, and caret width for the same span.

use std::collections::HashMap;

use crate::lexer::{FileId, ROOT_FILE, Span};
pub use quilon_rt::{MAX_PATH_WIDTH, shorten_path};

/// Byte offset of the start of every line in one source text.
///
/// Built once per file, because resolving a position is not a one-off: codegen resolves one
/// per call site that takes a `Site`. Walking the text from offset 0 each time made that
/// quadratic in file size — a file with a couple of thousand assertions near its end spent
/// seconds re-scanning its own prefix. A binary search over this table is `O(log lines)`.
#[derive(Debug, Clone)]
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    /// The 1-based line containing `offset`, and that line's start offset. An offset past
    /// the end of the text belongs to the last line, as `Span::line_col` also clamps.
    fn line_at(&self, offset: usize) -> (usize, usize) {
        let line = match self.starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(next) => next - 1,
        };
        (line + 1, self.starts[line])
    }
}

/// One file's identity: how to name it to a human, its full text, and the line table that
/// makes resolving a position in it cheap.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// The file as a reader should see it named — the path as given on the command line
    /// for the root file, the resolved path for a `<< "file.qn"` import, and the dotted
    /// module name (`core.test`) for a bundled built-in module.
    pub path: String,
    pub text: String,
    lines: LineIndex,
}

/// Every file in one compilation, keyed by the `FileId` its spans carry.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    files: HashMap<FileId, SourceFile>,
}

/// A span resolved to a human-readable position: which file and line it starts on, the
/// text of that line, and how many characters of it the span covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column, in characters.
    pub column: usize,
    /// The text of `line`, without its trailing newline — `None` when there is no such
    /// line to show (an empty file, or a position at the very end of one that ends in a
    /// newline), which is a report with no source snippet rather than an empty one.
    pub excerpt: Option<String>,
    /// Caret width: the span's character length, clamped to what remains on `line` and to
    /// at least 1 — so a zero-width span is still pointed at.
    pub width: usize,
}

impl Location {
    /// The position of a span the compiler has no source for — a program assembled in
    /// memory rather than read from a file. An EMPTY `path` is the "unknown" signal; the
    /// position stays 1-based so that arithmetic on it (a caret lead of `column - 1`)
    /// is well defined for every reader.
    pub fn unknown() -> Self {
        Self {
            path: String::new(),
            line: 1,
            column: 1,
            excerpt: None,
            width: 1,
        }
    }
}

impl SourceMap {
    /// Record `file`'s identity. Later inserts of the same id win, which is what makes
    /// [`set_root`](Self::set_root) able to name the root file after linking.
    pub fn insert(&mut self, file: FileId, path: impl Into<String>, text: impl Into<String>) {
        let text = text.into();
        let lines = LineIndex::new(&text);
        self.files.insert(
            file,
            SourceFile {
                path: path.into(),
                text,
                lines,
            },
        );
    }

    /// Record the root (command-line) file — the one whose spans carry [`ROOT_FILE`].
    pub fn set_root(&mut self, path: impl Into<String>, text: impl Into<String>) {
        self.insert(ROOT_FILE, path, text);
    }

    fn get(&self, file: FileId) -> Option<&SourceFile> {
        self.files.get(&file)
    }

    /// Every file in the map, as `(FileId, &SourceFile)` pairs in no particular order. Used by
    /// debug-info emission to build a `DIFile` and line table for each source a span can point
    /// into — the root file and every imported module.
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files.iter().map(|(id, file)| (*id, file))
    }

    /// One file's text, or `None` when that file is not in the map.
    pub fn get_text(&self, file: FileId) -> Option<&str> {
        self.get(file).map(|f| f.text.as_str())
    }

    /// The root file's text, or `""` when no root was recorded (an in-memory program).
    pub fn root_text(&self) -> &str {
        self.get(ROOT_FILE).map_or("", |f| f.text.as_str())
    }

    /// The root file's display path, or `""` when no root was recorded.
    pub fn root_path(&self) -> &str {
        self.get(ROOT_FILE).map_or("", |f| f.path.as_str())
    }

    /// Resolve `span` to a position in the file its `FileId` names. `None` when that file
    /// is not in the map — which is the case for a program assembled in memory rather than
    /// read from disk.
    pub fn locate(&self, span: &Span) -> Option<Location> {
        let file = self.get(span.file)?;
        Some(locate_with(&file.lines, &file.path, &file.text, span))
    }

    /// Resolve `span` the way [`locate`](Self::locate) does, falling back to the ROOT file
    /// when the span's own file is unknown — for a diagnostic, naming the file being
    /// compiled beats naming nothing.
    pub fn locate_or_root(&self, span: &Span) -> Option<Location> {
        self.locate(span).or_else(|| {
            let root = self.get(ROOT_FILE)?;
            Some(locate_with(&root.lines, &root.path, &root.text, span))
        })
    }
}

/// One document's byte-offset ⇄ protocol-position translation: zero-based lines and
/// **UTF-16 code unit** columns, which is what the Language Server Protocol requires of a
/// position. Distinct from [`Location`], whose one-based, character-counted columns are for
/// humans. Built once per document (it reuses the [`LineIndex`] machinery), and placed here
/// so span-to-position consumers — the language server today, debug tooling tomorrow —
/// share one translation.
pub struct DocumentPositions<'a> {
    text: &'a str,
    lines: LineIndex,
}

impl<'a> DocumentPositions<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: LineIndex::new(text),
        }
    }

    /// The zero-based line and UTF-16 column of `byte_offset`. An offset past the end of
    /// the text clamps to the final position; an offset inside a multi-byte character
    /// counts as that character's start.
    pub fn position_utf16(&self, byte_offset: usize) -> (u32, u32) {
        let offset = floor_char_boundary(self.text, byte_offset);
        let (line_one_based, line_start) = self.lines.line_at(offset);
        let column: usize = self.text[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum();
        ((line_one_based - 1) as u32, column as u32)
    }

    /// The byte offset of the position at zero-based `line` and UTF-16 column `character`.
    /// A line past the end of the text yields the text's length; a column past the end of
    /// its line stops at the line's end (before the newline), as the protocol specifies.
    pub fn byte_offset(&self, line: u32, character: u32) -> usize {
        let Some(&line_start) = self.lines.starts.get(line as usize) else {
            return self.text.len();
        };
        let mut remaining = character as usize;
        for (index, ch) in self.text[line_start..].char_indices() {
            if ch == '\n' || remaining == 0 {
                return line_start + index;
            }
            remaining = remaining.saturating_sub(ch.len_utf16());
        }
        self.text.len()
    }
}

/// Resolve `span` against one known file, without a map. `path` names the file and `source`
/// is its full text. Builds a line table for the single lookup, so prefer
/// [`SourceMap::locate`] when resolving many spans in the same file.
pub fn locate_in(path: &str, source: &str, span: &Span) -> Location {
    locate_with(&LineIndex::new(source), path, source, span)
}

fn locate_with(lines: &LineIndex, path: &str, source: &str, span: &Span) -> Location {
    let start = (span.start as usize).min(source.len());
    let (line, line_start) = lines.line_at(start);

    // The line's own text, and `None` when the position is past the last line that HAS
    // text (an empty file, or the offset just after a trailing newline).
    let excerpt = match line_start >= source.len() {
        true => None,
        false => Some(
            source[line_start..]
                .lines()
                .next()
                .unwrap_or("")
                .to_string(),
        ),
    };

    // Characters of this line that begin before `start` — counted the way
    // `Span::line_col` counts them, so the two never disagree about a column (including for
    // an offset that lands inside a multi-byte character, where slicing would panic).
    let column = source[line_start..]
        .char_indices()
        .take_while(|(offset, _)| line_start + offset < start)
        .count()
        + 1;

    // Caret run: the span's own character width, clamped to what is left on this line (a
    // multi-line span underlines only its first line) and to at least one caret.
    let span_chars = char_len(source, span.range());
    let remaining = excerpt
        .as_deref()
        .map_or(0, |line| line.chars().count())
        .saturating_sub(column - 1);
    let width = span_chars.clamp(1, remaining.max(1));

    Location {
        path: path.to_string(),
        line,
        column,
        excerpt,
        width,
    }
}

/// Number of `char`s in `source` over `range`, clamped to the source's bounds. Used for
/// the caret width so it counts scalar values, not bytes.
fn char_len(source: &str, range: std::ops::Range<usize>) -> usize {
    let start = floor_char_boundary(source, range.start);
    let end = floor_char_boundary(source, range.end).max(start);
    source[start..end].chars().count()
}

/// `index` moved back to the nearest character boundary at or before it, and clamped to the
/// text's length — so slicing at the result is always legal.
///
/// A span should already fall on boundaries, but it is arithmetic over byte offsets: one
/// landing inside a multi-byte character must not panic the compiler while it is reporting
/// something else. Flooring also matches how `Span::line_col` counts (a character counts
/// once its start is passed), which keeps the two in agreement.
fn floor_char_boundary(source: &str, index: usize) -> usize {
    let mut index = index.min(source.len());
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_reports_line_column_excerpt_and_width() {
        let src = "line one\nx = oops\nline three";
        let loc = locate_in("f.qn", src, &Span::in_root(13, 17));
        assert_eq!(loc.line, 2);
        assert_eq!(loc.column, 5);
        assert_eq!(loc.excerpt.as_deref(), Some("x = oops"));
        assert_eq!(loc.width, 4);
    }

    #[test]
    fn locate_survives_a_span_inside_a_character() {
        // A byte offset landing mid-character must not panic. Bytes 1..3 are the single
        // character `é`, so an offset of 2 is inside it.
        let src = "aé b";
        let inside = locate_in("f.qn", src, &Span::in_root(2, 3));
        assert_eq!((inside.line, inside.column), (1, Span::line_col(src, 2).1));
        assert_eq!(inside.excerpt.as_deref(), Some("aé b"));
    }

    #[test]
    fn locate_agrees_with_the_lexers_line_col() {
        // The line table and `Span::line_col` must never disagree — a diagnostic and a
        // `Site` would then point at different places in the same file.
        let src = "a = 1\nbé = 2\n\nc = 3";
        for offset in 0..=src.len() {
            let loc = locate_in("f.qn", src, &Span::in_root(offset as u32, offset as u32));
            assert_eq!(
                (loc.line, loc.column),
                Span::line_col(src, offset),
                "disagreement at byte {offset}"
            );
        }
    }

    #[test]
    fn locate_clamps_a_multiline_span_to_its_first_line() {
        let loc = locate_in("f.qn", "abc\ndef", &Span::in_root(0, 7));
        assert_eq!(loc.width, 3);
    }

    #[test]
    fn locate_widens_a_zero_width_span_to_one_caret() {
        let loc = locate_in("f.qn", "abc", &Span::in_root(1, 1));
        assert_eq!(loc.width, 1);
    }

    #[test]
    fn a_position_past_the_last_line_has_no_excerpt() {
        // Just after the trailing newline: there is no line to show.
        let loc = locate_in("f.qn", "abc\n", &Span::in_root(4, 4));
        assert_eq!(loc.excerpt, None);
        assert_eq!(loc.line, 2);
    }

    #[test]
    fn map_resolves_a_span_by_its_file_id() {
        let mut map = SourceMap::default();
        map.set_root("root.qn", "a = 1\n");
        map.insert(7, "core.test", "b = 2\nc = 3\n");
        assert_eq!(map.locate(&Span::in_root(0, 1)).unwrap().path, "root.qn");
        let imported = map.locate(&Span::in_file(6, 7, 7)).unwrap();
        assert_eq!((imported.path.as_str(), imported.line), ("core.test", 2));
        assert!(map.locate(&Span::in_file(0, 1, 99)).is_none());
    }

    #[test]
    fn locate_or_root_falls_back_to_the_file_being_compiled() {
        let mut map = SourceMap::default();
        map.set_root("root.qn", "a = 1\n");
        let unknown_file = Span::in_file(0, 1, 42);
        assert!(map.locate(&unknown_file).is_none());
        assert_eq!(map.locate_or_root(&unknown_file).unwrap().path, "root.qn");
        assert!(SourceMap::default().locate_or_root(&unknown_file).is_none());
    }

    #[test]
    fn utf16_positions_are_zero_based_line_and_column() {
        let positions = DocumentPositions::new("ab\ncd\n");
        assert_eq!(positions.position_utf16(0), (0, 0));
        assert_eq!(positions.position_utf16(1), (0, 1));
        assert_eq!(positions.position_utf16(3), (1, 0));
        assert_eq!(positions.position_utf16(4), (1, 1));
    }

    #[test]
    fn utf16_positions_count_code_units_not_bytes_or_characters() {
        // 'é' is 2 UTF-8 bytes but 1 UTF-16 unit; '😀' is 4 UTF-8 bytes and 2 UTF-16
        // units. The column after both must count 1 + 2 = 3 units, not bytes (6) and not
        // characters (2).
        let text = "é😀x";
        let positions = DocumentPositions::new(text);
        let offset_of_x = text.find('x').unwrap();
        assert_eq!(positions.position_utf16(offset_of_x), (0, 3));
    }

    #[test]
    fn utf16_positions_handle_a_multi_grapheme_cluster() {
        // The family emoji is ONE grapheme built from four scalars joined by zero-width
        // joiners: 25 UTF-8 bytes, 11 UTF-16 units. The protocol counts units.
        let text = "👨‍👩‍👧‍👦!";
        let positions = DocumentPositions::new(text);
        let offset_of_bang = text.find('!').unwrap();
        assert_eq!(positions.position_utf16(offset_of_bang), (0, 11));
    }

    #[test]
    fn utf16_position_clamps_past_the_end_and_floors_mid_character() {
        let positions = DocumentPositions::new("aé");
        assert_eq!(positions.position_utf16(99), (0, 2));
        // Byte 2 is inside 'é' (bytes 1..3): it floors to the character's start.
        assert_eq!(positions.position_utf16(2), (0, 1));
    }

    #[test]
    fn byte_offset_inverts_position_utf16() {
        let text = "a😀b\ncé d\n";
        let positions = DocumentPositions::new(text);
        for (offset, _) in text.char_indices() {
            let (line, character) = positions.position_utf16(offset);
            assert_eq!(
                positions.byte_offset(line, character),
                offset,
                "round trip failed at byte {offset}"
            );
        }
    }

    #[test]
    fn byte_offset_clamps_a_column_past_the_line_end() {
        let positions = DocumentPositions::new("ab\ncd");
        // Column 99 on line 0 stops before the newline.
        assert_eq!(positions.byte_offset(0, 99), 2);
        // A line past the end of the text yields the text's length.
        assert_eq!(positions.byte_offset(9, 0), 5);
    }

    #[test]
    fn root_text_and_path_are_empty_without_a_root() {
        let empty = SourceMap::default();
        assert_eq!(empty.root_text(), "");
        assert_eq!(empty.root_path(), "");
    }
}
