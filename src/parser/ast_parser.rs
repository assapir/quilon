// Parser implementation - simple recursive descent

use crate::ast::{
    BinaryOperator, Expression, FunctionDeclaration, Import, InterpolationPart, Item,
    MethodDeclaration, ModulePath, Parameter, Program, TypeDeclaration, TypeDefinition,
    UnaryOperator, VariableDeclaration,
};
use crate::diagnostic::Code;
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
    /// Every module-path spelling the `<<` lines above the cursor have bound — the short
    /// binding (`http`, a file's stem) and the full dotted path (`core.http`) — mapped to
    /// the module's canonical name. A same-line `Ident (. Ident)* . Ident` chain whose
    /// longest prefix is one of these is a QUALIFIED reference (`http.send`,
    /// `core.test.describe`) and parses as a single dotted name — see
    /// `try_parse_module_member`; the canonical value is what lets `at_test_block`
    /// recognize the harness's `describe` whatever spelling reaches it. Populated as
    /// imports are parsed, so (like every other name — the language has no hoisting) an
    /// import qualifies only the code below it.
    module_paths: std::collections::HashMap<String, String>,
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

/// How many parameters a function, method or lambda may declare — a rule of the LANGUAGE,
/// not a parser budget. Past this the arguments are a thing in their own right and want a
/// name: a record parameter says what the group is, keeps the call site readable, and is
/// itself unlimited. Enforced in `parse_parameter_list`, the one place every written
/// parameter list passes through; record types are deliberately not subject to it.
const MAX_PARAMETERS: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub code: Code,
    pub message: String,
    pub span: Span,
    /// The idiomatic fix, when there is one to show.
    pub help: Option<String>,
}

impl ParseError {
    pub fn new(code: Code, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span,
            help: None,
        }
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
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
            module_paths: std::collections::HashMap::new(),
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
            return Err(ParseError::new(
                Code::NestingTooDeep,
                self.current_span(),
                format!("expression nesting too deep: more than {MAX_NESTING_DEPTH} levels"),
            )
            .help("split the expression into named bindings"));
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
    /// `(1 + 2)` would parse as the call `f()(1 + 2)`, `b = a` followed by
    /// `[3, 4].each(...)` as the index `a[3, 4]`, and `b = a` followed by `{ x = 1 }`
    /// as the constructor `a { x = 1 }`. A `.` or operator at the start of a
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
            Err(ParseError::new(
                Code::UnexpectedToken,
                self.peek().span.clone(),
                format!(
                    "expected {}, found {}",
                    kind.describe(),
                    self.peek().kind.describe()
                ),
            ))
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
            Err(ParseError::new(
                Code::UnexpectedToken,
                self.peek().span.clone(),
                format!("expected a name, found {}", self.peek().kind.describe()),
            ))
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

    /// Whether the cursor could begin a qualified reference at all: an identifier with a
    /// same-line `.` right behind it, and at least one import above. The cheap gate every
    /// identifier passes before any chain is materialized.
    fn at_possible_module_chain(&self) -> bool {
        !self.module_paths.is_empty()
            && self.check(&TokenKind::Ident)
            && self.check_same_line_at(1, &TokenKind::Dot)
    }

    /// The maximal same-line `Ident (. Ident)* ` chain at the cursor, as its segment
    /// texts. Empty when the cursor is not on an identifier. Same-line only: a `.` that
    /// begins a source line continues an expression as a method chain, never a module
    /// path. Segment `i` sits at token offset `2*i` (its `.` at `2*i - 1`).
    fn dotted_chain_at_cursor(&self) -> Vec<String> {
        let mut segments = Vec::new();
        if !self.check(&TokenKind::Ident) {
            return segments;
        }
        segments.push(self.peek().text.clone());
        loop {
            let next = segments.len();
            if self.check_same_line_at(2 * next - 1, &TokenKind::Dot)
                && self.check_same_line_at(2 * next, &TokenKind::Ident)
            {
                segments.push(self.peek_ahead(2 * next).text.clone());
            } else {
                return segments;
            }
        }
    }

    /// The canonical module name and member of the longest proper chain prefix an import
    /// above has bound, with the member's segment index — or `None` when no prefix is a
    /// module path.
    fn resolve_chain<'s>(&self, segments: &'s [String]) -> Option<(&str, &'s str, usize)> {
        for prefix_len in (1..segments.len()).rev() {
            if let Some(canonical) = self.module_paths.get(&segments[..prefix_len].join(".")) {
                return Some((canonical, &segments[prefix_len], prefix_len));
            }
        }
        None
    }

    /// If the cursor begins a qualified reference — a dotted chain whose longest proper
    /// prefix is a module path bound by an import above (`http.send`,
    /// `core.test.describe`) — consume the path and ONE member segment, returning the
    /// joined dotted name and its span. Any further `.` continuation is the ordinary
    /// postfix grammar's (a field or method of the referenced value). `None` leaves the
    /// cursor untouched: the identifier is an ordinary name.
    fn try_parse_module_member(&mut self) -> Option<(String, Span)> {
        if !self.at_possible_module_chain() {
            return None;
        }
        let segments = self.dotted_chain_at_cursor();
        let (_, member, prefix_len) = self.resolve_chain(&segments)?;
        let name = format!("{}.{member}", segments[..prefix_len].join("."));
        let start = self.current_span().start;
        // The prefix's segments and dots, plus the member: 2 * prefix_len + 1.
        for _ in 0..(2 * prefix_len + 1) {
            self.advance();
        }
        let span = self.span(start, self.previous_span().end);
        Some((name, span))
    }

    /// Consume the identifier at the cursor as a reference: a qualified chain when an
    /// import above binds its prefix (`http.send`), the bare name otherwise. The flag
    /// says which — pattern position treats a qualified name as a constructor outright.
    fn parse_name_or_qualified(&mut self) -> (String, Span, bool) {
        if let Some((name, span)) = self.try_parse_module_member() {
            return (name, span, true);
        }
        let span = self.current_span();
        let name = self.peek().text.clone();
        self.advance();
        (name, span, false)
    }

    /// Record what an `<<` line binds, so the code below it can spell qualified names:
    /// the short binding and (for a dotted module) the full path, each mapped to the
    /// module's canonical name.
    fn register_import(&mut self, path: &ModulePath) {
        let Some(binding) = path.binding_name() else {
            return;
        };
        let canonical = match path {
            ModulePath::BuiltinDotted(parts) => parts.join("."),
            ModulePath::FilePath(_) => binding.clone(),
        };
        self.module_paths.insert(binding, canonical.clone());
        self.module_paths.insert(canonical.clone(), canonical);
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
