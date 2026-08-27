// Parser implementation - simple recursive descent

use crate::ast::{
    BinaryOperator, Expression, FunctionDeclaration, Import, InterpolationPart, Item,
    MethodDeclaration, ModulePath, Parameter, Program, TypeDeclaration, TypeDefinition,
    UnaryOperator, VariableDeclaration,
};
use crate::lexer::{FileId, Lexer, ROOT_FILE, Span, StrChunk, Token, TokenKind};

// The parser's rules live in child modules — one per part of the grammar — as further
// `impl<'a> Parser<'a>` blocks. Children of this file rather than siblings under
// `parser`, so the cursor state below stays private to the parser: a child can reach
// its ancestor's private items, a sibling could not.
mod exprs;
mod items;
mod lookahead;
mod patterns;
#[cfg(test)]
mod tests;
mod types;

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// The source these tokens were lexed from, stamped onto every span this parser
    /// builds (see `Parser::span`). All of a parse's tokens come from one file, so the
    /// id is read off the first token and never changes.
    file: FileId,
    /// Byte offset, within `file`, of position 0 of THIS parser's token stream. Zero for
    /// a whole-file parse; for a sub-parser re-lexing an interpolation hole it is the
    /// hole's absolute position in the file, so a nested hole's recorded (hole-relative)
    /// offset lifts back to a true file position — keeping every node's `(file, offset)`
    /// oracle key unique even for interpolation nested inside interpolation.
    span_base: usize,
    /// Current recursive-descent nesting depth, bounded by `MAX_NESTING_DEPTH`.
    /// Incremented on entry to each unbounded-recursion funnel and decremented on
    /// exit (see `nested`), so it always reflects the live parser stack. Guards
    /// against a stack overflow on hostile or machine-generated deeply nested input.
    depth: usize,
    /// While parsing a map-literal KEY, `ident => …` / `(…) => …` must read as a map entry
    /// (the `=>` is the "maps to" separator), NOT as a bare/parenthesized lambda. A map key
    /// is a hashable value and is never a function, so lambda detection is suppressed for
    /// the whole key expression while this is set (see `parse_fence_key`).
    suppress_lambda: bool,
}

/// Maximum recursive-descent nesting depth the parser accepts before it reports a
/// clean parse error instead of recursing until the native stack overflows.
///
/// Each source nesting level costs a full pass down the recursive grammar (for
/// expressions, the ~14-function precedence chain), so the real stack-overflow
/// threshold is a few hundred levels for expressions and higher for the lighter
/// type/pattern recursions. 128 is a deliberately conservative ceiling: it keeps
/// the deepest AST the parser will emit comfortably within reach of the *later*
/// passes (the type checker and codegen recurse over that AST too, and overflow
/// around a couple hundred levels), while remaining vastly deeper than any
/// hand-written program legitimately nests.
const MAX_NESTING_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            file: tokens.first().map_or(ROOT_FILE, |t| t.span.file),
            span_base: 0,
            suppress_lambda: false,
        }
    }

    /// A span over `start..end` in the file being parsed. Every AST node's span is built
    /// here (or cloned from a token), so a node from an imported module never keys the
    /// type oracle under another file's byte range.
    fn span(&self, start: u32, end: u32) -> Span {
        Span::in_file(start, end, self.file)
    }

    /// Run `f` one recursion level deeper, or fail loud if that would nest deeper
    /// than `MAX_NESTING_DEPTH`. Every parser entry point that can re-enter itself
    /// an unbounded number of times (`parse_expression`, `parse_type`, `parse_pattern`)
    /// routes through here, which pairs the depth increment with its matching
    /// decrement so no caller can leak or forget it. Returning a `ParseError`
    /// instead of recursing is what turns a would-be native stack overflow (abort +
    /// core dump) into an ordinary, source-located diagnostic.
    fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(ParseError {
                message: format!(
                    "expression nesting too deep (exceeded the maximum depth of {MAX_NESTING_DEPTH}); simplify or split this deeply nested expression"
                ),
                span: self.current_span(),
            });
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.pos += 1;
        }
        &self.tokens[self.pos - 1]
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    /// Like `check`, but false when the current token is the first token on its source
    /// line. This is the statement-boundary rule (the grammar's second line-aware rule,
    /// alongside the lexer's `>` classification): a line-first `(`, `[`, or `{` never
    /// continues the previous expression as a call, index, or record constructor — it
    /// begins a NEW statement. Call arguments, index brackets, and constructor braces
    /// must open on the same line as the expression they apply to. Without this,
    /// adjacent statements would fuse across the newline — `x = f()` followed by a line
    /// `(1 + 2) |> print` would parse as the call `f()(1 + 2)`, `b = a` followed by
    /// `[3, 4].each(...)` as the index `a[3, 4]`, and `b = a` followed by `{ x = 1 }`
    /// as the constructor `a { x = 1 }`. A `.`, `|>`, or operator at the start of a
    /// line still continues the expression, and an argument list opened on the callee's
    /// line may still span lines. Every postfix consumption of `(` / `[` / `{` must go
    /// through this guard.
    fn check_same_line(&self, kind: &TokenKind) -> bool {
        self.check_same_line_at(0, kind)
    }

    /// [`check_same_line`](Self::check_same_line), asked of the token `offset` past the
    /// cursor — so a rule that has to look one token ahead (is this identifier the start of
    /// a CALL?) still asks the statement-boundary question in one place.
    fn check_same_line_at(&self, offset: usize, kind: &TokenKind) -> bool {
        self.tokens
            .get(self.pos + offset)
            .is_some_and(|token| &token.kind == kind && !token.first_on_line)
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<(), ParseError> {
        if self.check(kind) {
            self.advance();
            Ok(())
        } else {
            Err(ParseError {
                message: format!("Expected {:?}, got {:?}", kind, self.peek().kind),
                span: self.peek().span.clone(),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        if self.check(&TokenKind::Ident) {
            let name = self.peek().text.clone();
            self.advance();
            Ok(name)
        } else if self.check(&TokenKind::EntryPoint) {
            // Allow ^ as a special function name (entry point)
            self.advance();
            Ok("^".to_string())
        } else {
            Err(ParseError {
                message: format!("Expected identifier, got {:?}", self.peek().kind),
                span: self.peek().span.clone(),
            })
        }
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len() || self.peek().kind == TokenKind::Eof
    }

    fn current_span(&self) -> Span {
        self.peek().span.clone()
    }

    fn previous_span(&self) -> Span {
        if self.pos > 0 {
            self.tokens[self.pos - 1].span.clone()
        } else {
            self.span(0, 0)
        }
    }
}

/// Parse a Quilon program from tokens
pub fn parse(tokens: &[Token]) -> Result<Program, ParseError> {
    Parser::parse(tokens)
}

/// Whether `name` is a Capitalized identifier (first char uppercase). Quilon's
/// convention is Capitalized = type/constructor, lowercase = value; this backs the
/// `/`-as-sum-separator-vs-division disambiguation and sum-variant name validation.
fn is_capitalized(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}
