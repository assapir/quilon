//! Speculative scans that decide which grammar rule applies before committing to it —
//! each looks ahead over tokens without consuming any.
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

impl<'a> Parser<'a> {
    /// Lookahead from the current position (just past `Name =`) to decide whether the
    /// RHS is a sum-type declaration rather than an ordinary value expression.
    ///
    /// A sum-type RHS is a `/`-separated list of variants, each a Capitalized
    /// constructor name optionally followed by a parenthesized payload-type list, with
    /// at least one top-level `/`. Disambiguation from division (LOCKED): a sum type
    /// requires that BOTH operands around the first top-level `/` are Capitalized
    /// constructor names. So `Red / Green` is a sum type, but `Min / 2` and `Min / x`
    /// are division (right operand isn't a Capitalized name), and `a / b` never matches
    /// (left operand isn't Capitalized either). A single `A` alone (no `/`) is a value
    /// binding, not a one-variant sum type.
    pub(super) fn looks_like_sum_decl(&self) -> bool {
        // The first token must be a Capitalized identifier (the first variant).
        if self.peek().kind != TokenKind::Ident || !is_capitalized(&self.peek().text) {
            return false;
        }

        let mut idx = 1; // we've conceptually consumed the first variant name
        // Optionally skip a balanced payload list `( ... )` after the first variant.
        if self.peek_ahead(idx).kind == TokenKind::ParenOpen {
            let mut depth = 0usize;
            loop {
                match self.peek_ahead(idx).kind {
                    TokenKind::ParenOpen => depth += 1,
                    TokenKind::ParenClose => {
                        depth -= 1;
                        if depth == 0 {
                            idx += 1;
                            break;
                        }
                    }
                    TokenKind::Eof => return false,
                    _ => {}
                }
                idx += 1;
            }
        }
        // Decisive signal: a top-level `/` whose right operand is ALSO a Capitalized
        // constructor name. Requiring both sides Capitalized is what keeps `Min / 2`
        // and `Total / count` as division rather than a misparsed sum type.
        if self.peek_ahead(idx).kind != TokenKind::Slash {
            return false;
        }
        let after_slash = self.peek_ahead(idx + 1);
        after_slash.kind == TokenKind::Ident && is_capitalized(&after_slash.text)
    }

    pub(super) fn peek_ahead(&self, offset: usize) -> &Token {
        let pos = self.pos + offset;
        if pos < self.tokens.len() {
            &self.tokens[pos]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    /// Whether the current operator token actually begins a top-level operator
    /// DEFINITION (`op = ...`) rather than continuing the current expression. The
    /// grammar is newline-insensitive, so without this an expression-bodied item
    /// followed by an operator overload — `x = 5` then `+ = (a, b) => …` — would let
    /// the additive parser swallow the `+` as `5 + …`. An operator immediately
    /// followed by `=` (Assign) is never a binary use (its right operand would be
    /// `=`), so we stop and let `parse_item` pick up the operator definition.
    pub(super) fn at_operator_definition(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Eq
                | TokenKind::Ne
                | TokenKind::Le
                | TokenKind::Ge
        ) && self.peek_ahead(1).kind == TokenKind::Assign
    }

    /// At a bare identifier in primary position, is this the start of a single-parameter
    /// lambda (`x => …` or `x :: Type => …`) rather than a plain value reference? We peek
    /// past the ident: a directly-following `=>` is a lambda; an `::` introduces a typed
    /// parameter, so we scan the (single) type annotation and require a `=>` after it.
    pub(super) fn looks_like_bare_lambda(&self) -> bool {
        debug_assert!(self.check(&TokenKind::Ident));
        // Inside a map-literal key, `ident => …` is a map entry, not a lambda.
        if self.suppress_lambda {
            return false;
        }
        match &self.peek_ahead(1).kind {
            TokenKind::Arrow => true,
            TokenKind::TypeAnnotation => {
                // `name :: <type> =>` — find the `=>` that closes the annotation. Types
                // here are simple (a name with optional `{ … }` generic args); the first
                // top-level `=>` after the `::` ends the param list of a lambda. A `<`
                // block or anything else means it was not a lambda parameter.
                let mut idx = 2;
                let mut brace_depth = 0i32;
                while idx < 40 {
                    match &self.peek_ahead(idx).kind {
                        TokenKind::BraceOpen => brace_depth += 1,
                        TokenKind::BraceClose => brace_depth -= 1,
                        TokenKind::Arrow if brace_depth == 0 => return true,
                        TokenKind::Eof => return false,
                        // A return-arrow, block, comma, etc. at depth 0 means this `::`
                        // was a binding annotation, not a lambda param — bail out.
                        TokenKind::BlockOpen | TokenKind::Comma if brace_depth == 0 => {
                            return false;
                        }
                        _ => {}
                    }
                    idx += 1;
                }
                false
            }
            _ => false,
        }
    }

    /// At `(` in primary position, does a parenthesized parameter list (`(a, b) =>` /
    /// `() =>`) follow — making this a lambda — rather than a parenthesized expression?
    /// Scans to the matching `)` and checks for a following `=>` or `->` (return type).
    pub(super) fn paren_starts_lambda(&self) -> bool {
        debug_assert!(self.check(&TokenKind::ParenOpen));
        // Inside a map-literal key, `(…) => …` is a map entry, not a lambda.
        if self.suppress_lambda {
            return false;
        }
        let mut depth = 1;
        let mut idx = 1;
        while idx < 80 && depth > 0 {
            match &self.peek_ahead(idx).kind {
                TokenKind::ParenOpen => depth += 1,
                TokenKind::ParenClose => {
                    depth -= 1;
                    if depth == 0 {
                        let next = &self.peek_ahead(idx + 1).kind;
                        return *next == TokenKind::Arrow || *next == TokenKind::ReturnArrow;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            idx += 1;
        }
        false
    }

    // Helper methods

    /// If the current token is an operator usable as an overload-set name AND it is
    /// being *defined* (followed by `=`), return its symbol. This is how a user
    /// declares an operator overload, e.g. `== = (a :: P, b :: P) -> Bool => ...`.
    /// Requiring the following `=` keeps a stray leading operator from being mistaken
    /// for a definition. `<`/`>` (block delimiters) are intentionally excluded here —
    /// a top-level `< ... >` would be a block, never an operator name.
    pub(super) fn operator_def_name(&self) -> Option<String> {
        let sym = match self.peek().kind {
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Eq => "==",
            TokenKind::Ne => "!=",
            TokenKind::Le => "<=",
            TokenKind::Ge => ">=",
            _ => return None,
        };
        // Only a definition (`op = ...`); otherwise leave it for expression parsing.
        if self.peek_ahead(1).kind == TokenKind::Assign {
            Some(sym.to_string())
        } else {
            None
        }
    }
}
