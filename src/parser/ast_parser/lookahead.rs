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
    pub(super) fn looks_like_sum_declaration(&self) -> bool {
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
    /// DEFINITION (`operator = ...`) rather than continuing the current expression. The
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
                // `name :: <type> =>` — the annotation runs up to the `=>` that opens the
                // lambda body, so the scan reads the type's own bracketing (`(…)`, `[…]`,
                // `{…}`) and takes the first `=>` outside it as the lambda arrow. A map
                // type's `=>` sits inside a fence and is skipped. Anything that cannot
                // appear in a written type ends the scan — `Eof` among it, which is what
                // terminates on a truncated stream — and means this `::` was a binding
                // annotation, not a lambda parameter.
                let mut idx = 2;
                let mut depth = 0usize;
                loop {
                    let kind = &self.peek_ahead(idx).kind;
                    match kind {
                        TokenKind::Arrow if depth == 0 => return true,
                        TokenKind::ParenOpen | TokenKind::BracketOpen | TokenKind::BraceOpen => {
                            depth += 1;
                        }
                        TokenKind::ParenClose | TokenKind::BracketClose | TokenKind::BraceClose => {
                            if depth == 0 {
                                return false;
                            }
                            depth -= 1;
                        }
                        TokenKind::Comma if depth == 0 => return false,
                        _ if kind.appears_in_type() => {}
                        _ => return false,
                    }
                    idx += 1;
                }
            }
            _ => false,
        }
    }

    /// At `(` in primary position, does a parenthesized parameter list (`(a, b) =>` /
    /// `() =>`) follow — making this a lambda — rather than a parenthesized expression?
    pub(super) fn paren_starts_lambda(&self) -> bool {
        debug_assert!(self.check(&TokenKind::ParenOpen));
        // Inside a map-literal key, `(…) => …` is a map entry, not a lambda.
        !self.suppress_lambda && self.parameter_list_ahead()
    }

    /// With the cursor on `(`, does a parenthesized parameter list followed by `=>` or
    /// `->` start here — the shape shared by a lambda and a function declaration?
    ///
    /// A parameter list holds only names, `::` annotations, written types and the commas
    /// between them, so the scan stops at the first token outside that alphabet: `(1 + 2)`
    /// is decided at the `1`, and `Eof` ends it on a stream that never closes the paren.
    /// That bounds the scan by the parameter list itself rather than by a token distance,
    /// which is what lets a definition of any width — and one whose parameters are all
    /// annotated — still read as a function.
    ///
    /// Stopping early does not have to mean "not a parameter list". An expression has no
    /// `::` of its own, so a `::` between these parens can only be a parameter annotation
    /// — unless a `=>` has already opened a lambda body inside them (`(x :: Num => x + 1)`
    /// is a parenthesized lambda). An annotated list that stops on a stray token is
    /// therefore a MALFORMED parameter list, and saying so hands the error to
    /// `parse_parameter_list`, which names the offending token instead of blaming the
    /// first `::` from the expression side.
    pub(super) fn parameter_list_ahead(&self) -> bool {
        let mut depth = 1usize;
        let mut idx = 1;
        let mut annotated = false;
        let mut lambda_body = false;
        loop {
            let kind = &self.peek_ahead(idx).kind;
            match kind {
                TokenKind::ParenOpen => depth += 1,
                TokenKind::ParenClose => {
                    depth -= 1;
                    if depth == 0 {
                        let next = &self.peek_ahead(idx + 1).kind;
                        return *next == TokenKind::Arrow || *next == TokenKind::ReturnArrow;
                    }
                }
                TokenKind::TypeAnnotation => annotated |= depth == 1,
                TokenKind::Arrow => lambda_body |= depth == 1,
                _ if kind.appears_in_type() => {}
                _ => return annotated && !lambda_body,
            }
            idx += 1;
        }
    }

    // Helper methods

    /// If the current token is an operator usable as an overload-set name AND it is
    /// being *defined* (followed by `=`), return its symbol. This is how a user
    /// declares an operator overload, e.g. `== = (a :: P, b :: P) -> Bool => ...`.
    /// Requiring the following `=` keeps a stray leading operator from being mistaken
    /// for a definition. `<`/`>` (block delimiters) are intentionally excluded here —
    /// a top-level `< ... >` would be a block, never an operator name.
    /// The operator symbol at the cursor, if it is one — without asking what follows.
    pub(super) fn operator_symbol_name(&self) -> Option<String> {
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
        Some(sym.to_string())
    }

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
        // Only a definition (`operator = ...`); otherwise leave it for expression parsing.
        // Deliberately NOT `:=`: an operator that mutates its left operand has no call-site
        // enforcement (operator calls do not consult the setter set), so declaring one is a
        // parse error rather than surface that looks checked and is not. An `=` operator
        // whose body mutates `it` is still caught by the verifier.
        if self.peek_ahead(1).kind == TokenKind::Assign {
            Some(sym.to_string())
        } else {
            None
        }
    }
}
