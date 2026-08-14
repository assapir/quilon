// Token types for Quilon lexer

use logos::Logos;
use std::fmt;

/// Source code position span
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Translate a byte `offset` into `source` into a 1-based `(line, column)`.
    ///
    /// Columns count Unicode scalar values (chars), not bytes, so multi-byte
    /// characters before the offset advance the column by one each. An offset
    /// that lands inside a multi-byte char is rounded down to that char's start.
    /// An offset at or past the end of the source clamps to the final position.
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

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
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

#[derive(Logos, Debug, Clone, PartialEq, Eq, Hash)]
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

    // Keywords
    #[token("if")]
    If,

    #[token("while")]
    While,

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

    // `>` is reclassified after lexing (see `Lexer::tokenize`): it stays `BlockClose`
    // when it is the last token on its line (`>` + optional `[ \t]*` + newline/EOF),
    // and becomes the greater-than operator `Gt` otherwise. This lets a bare `a > b`
    // work everywhere while a block still closes on a line-final `>`.
    #[token(">")]
    BlockClose,

    /// The greater-than comparison operator. Produced from a `>` that is NOT the last
    /// token on its line (see `BlockClose`). `<` is always `BlockOpen`; less-than is
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

/// Lex a whole string literal, starting just after the opening `"` (which the `#[token]`
/// already matched). Scans to the matching close quote, decoding escapes, collapsing a
/// doubled backtick ` `` ` to one literal backtick, and splitting off backtick
/// interpolation holes as raw expression sources. Consumes the scanned bytes (including
/// the close quote) via `lex.bump`. Returns `None` (a lexer error) on an unterminated
/// string, an unterminated hole, an empty hole, or an invalid escape.
///
/// Inside a hole the scanner walks over any nested string literal (respecting `\"`), so a
/// hole may itself contain a string with its own interpolation (`"sum `f("a")`"`); the
/// nested string's own holes are handled when the parser re-lexes the hole source.
fn lex_string(lex: &mut logos::Lexer<TokenKind>) -> Option<Vec<StrChunk>> {
    // Absolute byte offset of the first content byte (just past the opening quote).
    let base = lex.span().end;
    let rem = lex.remainder();
    let bytes = rem.as_bytes();
    let mut i = 0usize;
    let mut chunks: Vec<StrChunk> = Vec::new();
    let mut lit = String::new();

    loop {
        if i >= bytes.len() {
            return None; // unterminated string
        }
        match bytes[i] {
            b'"' => {
                i += 1; // consume the closing quote
                break;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return None;
                }
                match bytes[i] {
                    b'n' => lit.push('\n'),
                    b'r' => lit.push('\r'),
                    b't' => lit.push('\t'),
                    b'"' => lit.push('"'),
                    b'\\' => lit.push('\\'),
                    b'<' => lit.push('<'),
                    _ => return None, // invalid escape
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
                    loop {
                        if i >= bytes.len() {
                            return None; // unterminated hole
                        }
                        match bytes[i] {
                            b'`' => break, // closing backtick
                            b'"' => {
                                // Skip a nested string literal so its quotes/backticks
                                // don't end the hole; escapes are honored.
                                i += 1;
                                loop {
                                    if i >= bytes.len() {
                                        return None;
                                    }
                                    match bytes[i] {
                                        b'\\' => i += 2,
                                        b'"' => {
                                            i += 1;
                                            break;
                                        }
                                        _ => i += 1,
                                    }
                                }
                            }
                            _ => i += 1,
                        }
                    }
                    let src = rem[hole_start..i].to_string();
                    if src.trim().is_empty() {
                        return None; // empty hole `` `` `` is not an interpolation
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
    Some(chunks)
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Number(n) => write!(f, "Number({})", n.0),
            TokenKind::String(chunks) => {
                write!(f, "String(")?;
                for chunk in chunks {
                    match chunk {
                        StrChunk::Lit(s) => write!(f, "{}", s)?,
                        StrChunk::Hole { src, .. } => write!(f, "`{}`", src)?,
                    }
                }
                write!(f, ")")
            }
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::If => write!(f, "if"),
            TokenKind::While => write!(f, "while"),
            TokenKind::Underscore => write!(f, "_"),
            TokenKind::Ident => write!(f, "Ident"),
            TokenKind::Assign => write!(f, "="),
            TokenKind::MutAssign => write!(f, ":="),
            TokenKind::Arrow => write!(f, "=>"),
            TokenKind::ReturnArrow => write!(f, "->"),
            TokenKind::LeftArrow => write!(f, "<-"),
            TokenKind::TypeAnnotation => write!(f, "::"),
            TokenKind::Pipeline => write!(f, "|>"),
            TokenKind::EntryPoint => write!(f, "^"),
            TokenKind::Unit => write!(f, "$"),
            TokenKind::Export => write!(f, ">>"),
            TokenKind::Import => write!(f, "<<"),
            TokenKind::Question => write!(f, "?"),
            TokenKind::Pipe => write!(f, "|"),
            TokenKind::BlockOpen => write!(f, "<"),
            TokenKind::BlockClose => write!(f, ">"),
            TokenKind::Gt => write!(f, ">"),
            TokenKind::BraceOpen => write!(f, "{{"),
            TokenKind::BraceClose => write!(f, "}}"),
            TokenKind::ParenOpen => write!(f, "("),
            TokenKind::ParenClose => write!(f, ")"),
            TokenKind::BracketOpen => write!(f, "["),
            TokenKind::BracketClose => write!(f, "]"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Dot => write!(f, "."),
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::Eq => write!(f, "=="),
            TokenKind::Ne => write!(f, "!="),
            TokenKind::Le => write!(f, "<="),
            TokenKind::Ge => write!(f, ">="),
            TokenKind::And => write!(f, "&&"),
            TokenKind::Or => write!(f, "||"),
            TokenKind::Not => write!(f, "!"),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Backtick => write!(f, "`"),
            TokenKind::Eof => write!(f, "EOF"),
        }
    }
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

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.span)
    }
}
