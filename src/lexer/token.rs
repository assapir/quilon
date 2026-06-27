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

#[derive(Logos, Debug, Clone, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\r\n]+")] // Skip whitespace
#[logos(skip("~[^\n]*", allow_greedy = true))] // Skip comments (rest of line)
pub enum TokenKind {
    // Literals
    #[regex(r"[0-9]+\.?[0-9]*", |lex| lex.slice().parse().ok().map(NumLit))]
    Number(NumLit),

    #[regex(r#""(\\.|[^"\\])*""#, parse_string)]
    String(String),

    #[token("true")]
    True,

    #[token("false")]
    False,

    // Keywords
    #[token("if")]
    If,

    #[token("while")]
    While,

    #[token("for")]
    For,

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

    #[token(">")]
    BlockClose,

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

    // End of file
    Eof,
}

/// Parse string with escape sequences and interpolation
fn parse_string(lex: &mut logos::Lexer<TokenKind>) -> Option<String> {
    let s = lex.slice();
    // Remove quotes
    let content = &s[1..s.len() - 1];

    let mut result = String::new();
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some('<') => result.push('<'),
                _ => return None,
            }
        } else {
            result.push(ch);
        }
    }

    Some(result)
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Number(n) => write!(f, "Number({})", n.0),
            TokenKind::String(s) => write!(f, "String(\"{}\")", s),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::If => write!(f, "if"),
            TokenKind::While => write!(f, "while"),
            TokenKind::For => write!(f, "for"),
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
            TokenKind::Eof => write!(f, "EOF"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span, text: String) -> Self {
        Self { kind, span, text }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.span)
    }
}
