// Token types for Quilon lexer

use logos::Logos;

/// Which source a [`Span`]'s byte offsets index into. `ROOT_FILE` is the file the
/// compiler was invoked on; every `<<`-loaded module gets its own id from the module
/// loader.
pub type FileId = u32;

/// The source the compiler was invoked on, as opposed to an imported module.
pub const ROOT_FILE: FileId = 0;

/// The file id of a node the compiler built rather than read — today the entry point
/// `quilon test` synthesizes around a file's test blocks. Spans key the type oracle, so
/// giving synthesized nodes a file of their own is what keeps them from colliding with the
/// real nodes of any module; their offsets then only have to be distinct from each other.
/// No source is registered under it, so a diagnostic on such a span names the root file.
pub const SYNTHESIZED_FILE: FileId = FileId::MAX;

/// Source code position span: a byte range within ONE source file.
///
/// `file` says which source `start`/`end` index into. Every module is lexed on its own,
/// so offsets restart at 0 in each one and a bare byte range is ambiguous across a
/// program that imports anything: two expressions in two files routinely share a range.
/// Spans are the key of the type checker's per-expression table (the type oracle codegen
/// reads back), so that ambiguity would make one module's inferred type answer for
/// another's expression — carrying the file id keeps every node's key unique.
///
/// Offsets are 32-bit (source files are far below 4 GiB, and a `Span` per AST node rides
/// the parser's and checker's recursive frames, where every byte of width costs nesting
/// headroom).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub file: FileId,
}

impl Span {
    /// A span in the root file — named for the claim it makes, since claiming the wrong
    /// file is exactly what collides in the type table. Anything built while processing
    /// an imported module (or any other source) must use [`Span::in_file`].
    pub fn in_root(start: u32, end: u32) -> Self {
        Self {
            start,
            end,
            file: ROOT_FILE,
        }
    }

    /// A span in the source identified by `file`.
    pub fn in_file(start: u32, end: u32, file: FileId) -> Self {
        Self { start, end, file }
    }

    /// The span's byte range, for slicing the source it came from.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Translate a byte `offset` into `source` into a 1-based `(line, column)`.
    ///
    /// Columns count Unicode scalar values (chars), not bytes, so multi-byte
    /// characters before the offset advance the column by one each. An offset
    /// that lands inside a multi-byte char is rounded down to that char's start.
    /// An offset at or past the end of the source clamps to the final position.
    ///
    /// This is the straightforward scan-from-the-top definition. Everything that resolves a
    /// position in anger goes through [`crate::source_map`], which keeps a per-file line
    /// index because it answers one query per call site rather than one per compilation —
    /// and whose tests check it against THIS function for every byte offset, so the fast
    /// path can never quietly disagree with the obvious one.
    pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(source.len());
        let mut line = 1;
        let mut col = 1;
        for (idx, ch) in source.char_indices() {
            if idx >= offset {
                return (line, col);
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }
}

/// Wrapper for f64 to implement Eq/Hash for parser
#[derive(Debug, Clone, Copy)]
pub struct NumLit(pub f64);

impl PartialEq for NumLit {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for NumLit {}

impl std::hash::Hash for NumLit {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

/// One piece of a (possibly interpolated) string literal, produced by the lexer.
///
/// A plain string `"hello"` lexes to a single `Lit("hello")`. Backtick holes split the
/// literal into interleaved `Lit` text and `Hole` expression sources: `"hi `user.name`!"`
/// lexes to `[Lit("hi "), Hole { src: "user.name", .. }, Lit("!")]`. A doubled backtick
/// ` `` ` inside a string is a single literal backtick (never a hole). The parser re-lexes
/// and parses each `Hole.src` into an expression, offset by `Hole.offset` so the hole's
/// AST spans stay in the original file's coordinate system (the type oracle keys by span).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StrChunk {
    /// A run of literal text, with escapes already decoded and ` `` ` collapsed to `` ` ``.
    Lit(String),
    /// The raw source of an interpolation hole (the expression between two backticks),
    /// with `offset` its absolute byte position in the whole source file.
    Hole { src: String, offset: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TokenLexError {
    #[default]
    InvalidToken,
    UnterminatedString,
}

#[derive(Logos, Debug, Clone, PartialEq, Eq, Hash)]
#[logos(error = TokenLexError)]
#[logos(skip r"[ \t\r\n]+")] // Skip whitespace
#[logos(skip("~[^\n]*", allow_greedy = true))] // Skip comments (rest of line)
pub enum TokenKind {
    // Literals
    #[regex(r"[0-9]+\.?[0-9]*", |lex| lex.slice().parse().ok().map(NumLit))]
    Number(NumLit),

    // A string literal, lexed whole (including any backtick interpolation holes) by
    // `lex_string`. The chunks are literal text interleaved with hole expression sources;
    // a plain string is a single `StrChunk::Lit`. Triggered on the opening `"`, then the
    // callback scans to the matching close quote (holes and nested strings included).
    #[token("\"", lex_string)]
    String(Vec<StrChunk>),

    #[token("true")]
    True,

    #[token("false")]
    False,

    #[token("_")]
    Underscore,

    // Identifiers (but not just "_")
    #[regex(r"_[a-zA-Z0-9_]+|[a-zA-Z][a-zA-Z0-9_]*")]
    Ident,

    // Operators and delimiters
    #[token("=")]
    Assign,

    // Mutable bind/reassign operator (replaces the old `mut` keyword).
    #[token(":=")]
    MutAssign,

    #[token("=>")]
    Arrow,

    #[token("->")]
    ReturnArrow,

    #[token("<-")]
    LeftArrow,

    #[token("::")]
    TypeAnnotation,

    #[token("|>")]
    Pipeline,

    // Marks a leaf IO primitive in the corelib (`@sleep`, a future `@get`): the only
    // marker in the colorless implicit-futures model. Lexed as its own token; the parser
    // fuses `@` + the following identifier into the primitive's name (`@sleep`), both at
    // its corelib declaration and at every call site. Never valid on user declarations.
    #[token("@")]
    At,

    #[token("^")]
    EntryPoint,

    // The unit type and its sole value, written `$` (analogous to `()` in Rust/ML).
    // Same symbol in type position (`-> $`) and value position (`$`).
    #[token("$")]
    Unit,

    #[token(">>")]
    Export,

    #[token("<<")]
    Import,

    #[token("?")]
    Question,

    #[token("|")]
    Pipe,

    #[token("<")]
    BlockOpen,

    /// The `< … >` block close — what a `>` is **by default**. `Lexer::reclassify_gt`
    /// turns it into [`TokenKind::Gt`] only when the next token is on the same line and
    /// [starts an operand](TokenKind::starts_operand), so a block closes before a `)`,
    /// `]`, `}`, `,`, `.`, a `~` comment, or the end of the line — which is what lets
    /// `f(() => < … >)` fit on one line — while `a > b` stays a comparison.
    #[token(">")]
    BlockClose,

    /// The greater-than comparison operator: a `>` with an operand after it on the same
    /// line (see [`TokenKind::BlockClose`]). `<` is always `BlockOpen`; less-than is
    /// recovered in the parser, where a `<` after a complete operand can only be `Lt`.
    Gt,

    #[token("{")]
    BraceOpen,

    #[token("}")]
    BraceClose,

    #[token("(")]
    ParenOpen,

    #[token(")")]
    ParenClose,

    #[token("[")]
    BracketOpen,

    #[token("]")]
    BracketClose,

    #[token(",")]
    Comma,

    #[token(".")]
    Dot,

    // Arithmetic operators
    #[token("+")]
    Plus,

    #[token("-")]
    Minus,

    #[token("*")]
    Star,

    #[token("/")]
    Slash,

    #[token("%")]
    Percent,

    // Set intersection, written `+-` or `-+` — the two spellings are the SAME symmetric
    // operator. Lexed as single tokens; logos maximal munch makes the two-char form win
    // over `+`/`-` when the characters are adjacent. With whitespace, `a + -b` still
    // lexes as `Plus`/`Minus` (plus-then-negate on `Num`).
    #[token("+-")]
    PlusMinus,

    #[token("-+")]
    MinusPlus,

    // Comparison operators
    #[token("==")]
    Eq,

    #[token("!=")]
    Ne,

    #[token("<=")]
    Le,

    #[token(">=")]
    Ge,

    // Logical operators
    #[token("&&")]
    And,

    #[token("||")]
    Or,

    #[token("!")]
    Not,

    #[token(":")]
    Colon,

    // The render operator `` ` ``. Only ever seen OUTSIDE a string literal (backticks
    // *inside* a `"..."` are consumed whole by `lex_string` as interpolation holes), so
    // there is no ambiguity: a bare `` ` `` token is the overloadable render operator,
    // used to define a type's own rendering (`` ` = () -> Text => ... ``).
    #[token("`")]
    Backtick,

    // End of file
    Eof,
}

impl TokenKind {
    /// Whether this kind can be the first token of an operand — exactly the parser's
    /// `parse_unary`/`parse_primary` entry set, which a `debug_assert!` there keeps in
    /// step with this list. It is what makes a preceding `>` a comparison rather than a
    /// [block close](TokenKind::BlockClose). `<` is deliberately absent: a block is not
    /// an operand (in operand position a `<` is less-than), so a `>` before a `<` closes.
    pub fn starts_operand(&self) -> bool {
        matches!(
            self,
            TokenKind::Number(_)
                | TokenKind::String(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Unit
                | TokenKind::At
                | TokenKind::Ident
                | TokenKind::ParenOpen
                | TokenKind::BracketOpen
                | TokenKind::BraceOpen
                | TokenKind::Minus
                | TokenKind::Not
        )
    }

    /// Whether this kind can appear inside a WRITTEN TYPE — the alphabet the parser's
    /// `parse_type` accepts, which a `debug_assert!` there keeps in step with this list.
    /// A speculative scan over a `::` annotation or a parameter list stops at the first
    /// token outside this set, which is how such a scan is bounded by the construct it is
    /// reading rather than by a token count.
    pub fn appears_in_type(&self) -> bool {
        matches!(
            self,
            TokenKind::Ident
                | TokenKind::Unit
                | TokenKind::ParenOpen
                | TokenKind::ParenClose
                | TokenKind::BracketOpen
                | TokenKind::BracketClose
                | TokenKind::BraceOpen
                | TokenKind::BraceClose
                | TokenKind::Comma
                | TokenKind::Pipe
                | TokenKind::ReturnArrow
                | TokenKind::Arrow
        )
    }
}

/// Lex a whole string literal, starting just after the opening `"` (which the `#[token]`
/// already matched). Scans to the matching close quote, decoding escapes, collapsing a
/// doubled backtick ` `` ` to one literal backtick, and splitting off backtick
/// interpolation holes as raw expression sources. Consumes the scanned bytes (including
/// the close quote) via `lex.bump`. Returns `None` (a lexer error) on an unterminated
/// string, an unterminated hole, an empty hole, or an invalid escape.
///
/// A hole's bounds are found by `scan_hole_end`, which skips nested string literals whole
/// (and their own holes, recursively), so a hole may itself contain a string with its own
/// interpolation (`"sum `f("a")`"`) at any nesting depth; each nested string's holes are
/// handled when the parser re-lexes the hole source.
fn lex_string(lex: &mut logos::Lexer<TokenKind>) -> Result<Vec<StrChunk>, TokenLexError> {
    // Absolute byte offset of the first content byte (just past the opening quote).
    let base = lex.span().end;
    let rem = lex.remainder();
    let bytes = rem.as_bytes();
    let mut i = 0usize;
    let mut chunks: Vec<StrChunk> = Vec::new();
    let mut lit = String::new();

    loop {
        if i >= bytes.len() {
            return Err(TokenLexError::InvalidToken); // unterminated string at EOF
        }
        match bytes[i] {
            b'\n' | b'\r' => return Err(TokenLexError::UnterminatedString),
            b'"' => {
                i += 1; // consume the closing quote
                break;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return Err(TokenLexError::InvalidToken);
                }
                match bytes[i] {
                    b'\n' | b'\r' => return Err(TokenLexError::UnterminatedString),
                    b'n' => lit.push('\n'),
                    b'r' => lit.push('\r'),
                    b't' => lit.push('\t'),
                    // `\e` is the ESC byte (U+001B), the lead-in of every ANSI terminal
                    // sequence — the one control character a program needs to emit color
                    // and cannot otherwise write, since a raw ESC in a source file is
                    // invisible.
                    b'e' => lit.push('\u{1b}'),
                    b'"' => lit.push('"'),
                    b'\\' => lit.push('\\'),
                    b'<' => lit.push('<'),
                    _ => return Err(TokenLexError::InvalidToken),
                }
                i += 1;
            }
            b'`' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    // Doubled backtick -> one literal backtick (never a hole).
                    lit.push('`');
                    i += 2;
                } else {
                    // Opening backtick: flush the pending literal and scan the hole.
                    if !lit.is_empty() {
                        chunks.push(StrChunk::Lit(std::mem::take(&mut lit)));
                    }
                    i += 1; // consume the opening backtick
                    let hole_start = i;
                    i = scan_hole_end(bytes, i)?; // -> index of the closing backtick
                    let src = rem[hole_start..i].to_string();
                    if src.trim().is_empty() {
                        return Err(TokenLexError::InvalidToken); // empty hole `` `` `` is not an interpolation
                    }
                    chunks.push(StrChunk::Hole {
                        src,
                        offset: base + hole_start,
                    });
                    i += 1; // consume the closing backtick
                }
            }
            _ => {
                // A normal (possibly multi-byte) character copied verbatim.
                let ch = rem[i..].chars().next().unwrap();
                lit.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    // Keep a trailing literal, and keep a single empty literal for the empty string `""`.
    if !lit.is_empty() || chunks.is_empty() {
        chunks.push(StrChunk::Lit(lit));
    }
    lex.bump(i);
    Ok(chunks)
}

/// Scan from `i` (the first byte of a hole's content, just past its opening backtick) to
/// the index of the hole's matching CLOSING backtick. A nested string literal inside the
/// hole is skipped whole — including that string's own interpolation holes, recursively —
/// so a `"` or `` ` `` within a nested string never ends the hole. An error if unterminated.
fn scan_hole_end(bytes: &[u8], mut i: usize) -> Result<usize, TokenLexError> {
    while i < bytes.len() {
        match bytes[i] {
            b'\n' | b'\r' => return Err(TokenLexError::UnterminatedString),
            b'`' => return Ok(i),
            b'"' => i = scan_string_end(bytes, i)?,
            _ => i += 1,
        }
    }
    Err(TokenLexError::InvalidToken)
}

/// Scan from `i` (at an opening `"`) to the index JUST PAST the matching closing `"`,
/// honoring `\"` escapes, treating ` `` ` as a literal backtick, and recursing over any
/// interpolation holes inside (whose contents may contain further strings). An error if
/// unterminated. Used only to find bounds while skipping — string CONTENT is decoded when
/// the piece is actually lexed.
fn scan_string_end(bytes: &[u8], mut i: usize) -> Result<usize, TokenLexError> {
    i += 1; // past the opening quote
    while i < bytes.len() {
        match bytes[i] {
            b'\n' | b'\r' => return Err(TokenLexError::UnterminatedString),
            b'\\' => {
                if bytes
                    .get(i + 1)
                    .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
                {
                    return Err(TokenLexError::UnterminatedString);
                }
                i += 2; // escaped char (possibly the closing-quote-looking `\"`)
            }
            b'"' => return Ok(i + 1),
            b'`' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    i += 2; // doubled backtick -> one literal backtick
                } else {
                    i = scan_hole_end(bytes, i + 1)? + 1; // skip the hole and its close backtick
                }
            }
            _ => i += 1,
        }
    }
    Err(TokenLexError::InvalidToken)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
    /// Whether this token is the first token on its source line. Feeds the parser's
    /// line-first `(` / `[` statement-boundary rule (see `Parser::check_same_line`).
    pub first_on_line: bool,
}
