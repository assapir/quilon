//! `?`/`|` matching: the arms and the patterns they test.
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_match(&mut self, expression: Expression) -> Result<Expression, ParseError> {
        let start = expression.span().start;
        let mut arms = Vec::new();

        // Parse match arms: | pattern => body
        while self.check(&TokenKind::Pipe) {
            self.advance();

            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::Arrow)?;
            let body = self.parse_expression()?;
            let arm_span = self.span(pattern.span().start, body.span().end);

            arms.push(crate::ast::MatchArm {
                pattern,
                body,
                span: arm_span,
            });
        }

        if arms.is_empty() {
            return Err(ParseError {
                message: "Match expression must have at least one arm".to_string(),
                span: self.span(start, start),
            });
        }

        let end = arms.last().unwrap().span.end;

        Ok(Expression::Match {
            expression: Box::new(expression),
            arms,
            span: self.span(start, end),
        })
    }

    /// Depth-guarded entry point for pattern parsing. A constructor pattern's
    /// arguments recurse back into `parse_pattern` (`Ok(Ok(…))`), independently of
    /// the expression grammar, so nested patterns get the same `MAX_NESTING_DEPTH`
    /// bound to keep deeply nested patterns from overflowing the stack.
    pub(super) fn parse_pattern(&mut self) -> Result<crate::ast::Pattern, ParseError> {
        self.nested(Self::parse_pattern_inner)
    }

    pub(super) fn parse_pattern_inner(&mut self) -> Result<crate::ast::Pattern, ParseError> {
        use crate::ast::Pattern;

        let token = self.peek();

        match &token.kind {
            TokenKind::Ident => {
                // A qualified variant — `http.Get`, `http.Post(b)` — is always a
                // constructor pattern: dotted names reach an imported module's exports
                // and never bind. (A bare lowercase name still binds; a bare Capitalized
                // one is still a nullary constructor.)
                let bare_name = token.text.clone();
                let bare_span = token.span.clone();
                let (name, span, qualified) = match self.try_parse_module_member() {
                    Some((name, span)) => (name, span, true),
                    None => {
                        self.advance();
                        (bare_name, bare_span, false)
                    }
                };

                // Check if it's a constructor: Name(patterns) or Name pattern
                if self.check(&TokenKind::ParenOpen) {
                    self.advance();
                    let mut arguments = Vec::new();

                    if !self.check(&TokenKind::ParenClose) {
                        loop {
                            arguments.push(self.parse_pattern()?);
                            if !self.check(&TokenKind::Comma) {
                                break;
                            }
                            self.advance();
                        }
                    }

                    self.expect(&TokenKind::ParenClose)?;
                    let end = self.previous_span().end;

                    Ok(Pattern::Constructor {
                        name,
                        arguments,
                        span: self.span(span.start, end),
                    })
                } else if qualified || is_capitalized(&name) {
                    // A bare Capitalized name in pattern position is a nullary constructor
                    // (e.g. `| Red =>`), not a binding; a qualified name always is one.
                    // Lowercase bare names bind a value.
                    Ok(Pattern::Constructor {
                        name,
                        arguments: vec![],
                        span,
                    })
                } else {
                    // Just an identifier pattern (binds the scrutinee value)
                    Ok(Pattern::Identifier { name, span })
                }
            }
            TokenKind::Number(value) => {
                let value = value.0;
                let span = token.span.clone();
                self.advance();
                Ok(Pattern::Number { value, span })
            }
            TokenKind::Underscore => {
                let span = token.span.clone();
                self.advance();
                Ok(Pattern::Wildcard { span })
            }
            _ => Err(ParseError {
                message: format!("Expected pattern, got {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
    }
}
