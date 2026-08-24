//! The expression grammar: the precedence chain from assignment down to postfix and
//! primary, plus the literal forms (lambda, record, array, interpolated string).
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        // Single funnel for every nested expression: parens `(…)`, array elements,
        // record/constructor field values, block statements, lambda/ternary/spread
        // sub-expressions all re-enter here, so depth-guarding here bounds the whole
        // expression grammar's recursion — deep nesting fails loud, never crashes.
        self.nested(Self::parse_assignment)
    }

    /// Assignment is the lowest-precedence form. Parse a ternary; if it is a
    /// field-access path (`a.b` / `a.b.c`) immediately followed by `:=`, treat
    /// the whole thing as an in-place field write `target := value`. Anything
    /// else (including a bare `name := …`, which `parse_item` handles) falls
    /// straight through unchanged.
    pub(super) fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_ternary()?;

        if self.check(&TokenKind::MutAssign) && matches!(expr, Expr::FieldAccess { .. }) {
            self.advance(); // consume `:=`
            // Depth-guard the value: a `:=` chain (`a.x := b.y := …`) re-enters
            // `parse_assignment` directly, bypassing the `parse_expr` funnel.
            let value = self.nested(Self::parse_assignment)?;
            let span = self.span(expr.span().start, value.span().end);
            return Ok(Expr::FieldAssign {
                target: Box::new(expr),
                value: Box::new(value),
                span,
            });
        }

        Ok(expr)
    }

    pub(super) fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_logical_or()?;

        // Check for ? operator - could be ternary or pattern match
        if self.check(&TokenKind::Question) {
            self.advance();

            // Check if it's pattern match (next token is |) or ternary
            if self.check(&TokenKind::Pipe) {
                // Pattern match: expr ? | pattern => body | pattern => body
                return self.parse_match(expr);
            } else {
                // Ternary: expr ? then : else
                let then_expr = self.parse_expr()?;
                self.expect(&TokenKind::Colon)?;
                let else_expr = self.parse_expr()?;
                let span = self.span(expr.span().start, else_expr.span().end);

                return Ok(Expr::If {
                    condition: Box::new(expr),
                    then: Box::new(then_expr),
                    else_: Box::new(else_expr),
                    span,
                });
            }
        }

        Ok(expr)
    }

    pub(super) fn parse_logical_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_logical_and()?;

        while self.check(&TokenKind::Or) {
            self.advance();
            let right = self.parse_logical_and()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::Or,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_logical_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;

        while self.check(&TokenKind::And) {
            self.advance();
            let right = self.parse_equality()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::And,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;

        while let Some(op) = self.match_equality() {
            let right = self.parse_comparison()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_range()?;

        while let Some(op) = self.match_comparison() {
            let right = self.parse_range()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    /// Infix range `lo <- hi` → inclusive `[]Num` (see the `Expr::Range` node).
    /// Non-associative: consumes at most one `<-`, so `a <- b <- c` is rejected.
    pub(super) fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_pipeline()?;

        if self.check(&TokenKind::LeftArrow) {
            self.advance(); // consume `<-`
            let right = self.parse_pipeline()?;
            let span = self.span(left.span().start, right.span().end);
            return Ok(Expr::Range {
                start: Box::new(left),
                end: Box::new(right),
                span,
            });
        }

        Ok(left)
    }

    pub(super) fn parse_pipeline(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive()?;

        while self.check(&TokenKind::Pipeline) {
            self.advance();
            let right = self.parse_additive()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::Pipeline {
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative()?;

        while let Some(op) = self.match_additive() {
            let right = self.parse_multiplicative()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        while let Some(op) = self.match_multiplicative() {
            let right = self.parse_unary()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::Minus) {
            let start = self.current_span();
            self.advance();
            // Depth-guard the operand: a chain of prefix operators (`---…x`,
            // `!!!…x`) re-enters `parse_unary` without passing through `parse_expr`,
            // so bound it here too to keep deep chains from overflowing the stack.
            let expr = self.nested(Self::parse_unary)?;
            let span = self.span(start.start, expr.span().end);
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                span,
            });
        }

        if self.check(&TokenKind::Not) {
            let start = self.current_span();
            self.advance();
            let expr = self.nested(Self::parse_unary)?; // depth-guarded (see Neg branch)
            let span = self.span(start.start, expr.span().end);
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }

        self.parse_postfix()
    }

    /// Parse postfix continuations: `.field` / `.method(args)` / `[index]` / `(args)`.
    /// The `(` / `[` continuations are gated by `check_same_line` — a line-first `(`
    /// or `[` begins a new statement instead (a `.`-led line still chains). The record
    /// constructor `Ident { ... }` in `parse_primary` is gated by the same predicate.
    pub(super) fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            if self.check(&TokenKind::Dot) {
                self.advance();
                let field = self.expect_ident()?;

                // A same-line `(` makes this a method call: obj.method(args). A
                // line-first `(` leaves `.field` a plain field access and the `(...)`
                // begins the next statement.
                if self.check_same_line(&TokenKind::ParenOpen) {
                    // Method call: desugar obj.method(a, b) to method(obj, a, b)
                    self.advance(); // consume '('

                    // Parse arguments
                    let mut arguments = vec![expr]; // receiver is first argument

                    if !self.check(&TokenKind::ParenClose) {
                        loop {
                            arguments.push(self.parse_expr()?);
                            if !self.check(&TokenKind::Comma) {
                                break;
                            }
                            self.advance();
                        }
                    }

                    self.expect(&TokenKind::ParenClose)?;
                    let span = self.span(arguments[0].span().start, self.previous_span().end);

                    // Create function call with method name
                    expr = Expr::Call {
                        function: Box::new(Expr::Ident {
                            name: field,
                            span: span.clone(),
                        }),
                        arguments,
                        span,
                    };
                } else {
                    // Regular field access
                    let span = self.span(expr.span().start, self.previous_span().end);
                    expr = Expr::FieldAccess {
                        expr: Box::new(expr),
                        field,
                        span,
                    };
                }
            } else if self.check_same_line(&TokenKind::BracketOpen) {
                // Array indexing
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::BracketClose)?;
                let span = self.span(expr.span().start, self.previous_span().end);
                expr = Expr::Index {
                    expr: Box::new(expr),
                    index: Box::new(index),
                    span,
                };
            } else if self.check_same_line(&TokenKind::ParenOpen) {
                // Function call
                self.advance();
                let mut arguments = Vec::new();

                if !self.check(&TokenKind::ParenClose) {
                    loop {
                        arguments.push(self.parse_expr()?);
                        if !self.check(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }

                self.expect(&TokenKind::ParenClose)?;
                let span = self.span(expr.span().start, self.previous_span().end);
                expr = Expr::Call {
                    function: Box::new(expr),
                    arguments,
                    span,
                };
            } else {
                break;
            }
        }

        Ok(expr)
    }

    // Match helper functions
    pub(super) fn match_equality(&mut self) -> Option<BinOp> {
        if self.check(&TokenKind::Eq) {
            self.advance();
            Some(BinOp::Eq)
        } else if self.check(&TokenKind::Ne) {
            self.advance();
            Some(BinOp::Ne)
        } else {
            None
        }
    }

    pub(super) fn match_comparison(&mut self) -> Option<BinOp> {
        if self.at_operator_definition() {
            return None;
        }
        match &self.peek().kind {
            TokenKind::Le => {
                self.advance();
                Some(BinOp::Le)
            }
            TokenKind::Ge => {
                self.advance();
                Some(BinOp::Ge)
            }
            // `<` doubles as the block-open delimiter, but in comparison position
            // (after a complete left operand) a block can never start, so a bare
            // `<` here is unambiguously the less-than operator.
            TokenKind::BlockOpen => {
                self.advance();
                Some(BinOp::Lt)
            }
            // The lexer already distinguished a greater-than `>` (token `Gt`) from a
            // block-closing `>` (token `BlockClose`, only when line-final), so a `Gt`
            // here is unambiguously the operator.
            TokenKind::Gt => {
                self.advance();
                Some(BinOp::Gt)
            }
            _ => None,
        }
    }

    pub(super) fn match_additive(&mut self) -> Option<BinOp> {
        if self.at_operator_definition() {
            return None;
        }
        if self.check(&TokenKind::Plus) {
            self.advance();
            Some(BinOp::Add)
        } else if self.check(&TokenKind::Minus) {
            self.advance();
            Some(BinOp::Sub)
        } else {
            None
        }
    }

    pub(super) fn match_multiplicative(&mut self) -> Option<BinOp> {
        if self.at_operator_definition() {
            return None;
        }
        match &self.peek().kind {
            TokenKind::Star => {
                self.advance();
                Some(BinOp::Mul)
            }
            TokenKind::Slash => {
                self.advance();
                Some(BinOp::Div)
            }
            TokenKind::Percent => {
                self.advance();
                Some(BinOp::Mod)
            }
            // `+-` / `-+` — set intersection (symmetric); binds tighter than `+`/`-`
            // union/difference, mirroring arithmetic's product-over-sum precedence.
            TokenKind::PlusMinus | TokenKind::MinusPlus => {
                self.advance();
                Some(BinOp::SetIntersect)
            }
            _ => None,
        }
    }

    /// Build a string expression from the lexer's chunks. A single literal chunk is a
    /// plain `Expr::String`; any interpolation hole yields an `Expr::Interpolation` whose
    /// holes are re-lexed and parsed as expressions.
    pub(super) fn build_string_expr(
        &self,
        chunks: Vec<StrChunk>,
        span: Span,
    ) -> Result<Expr, ParseError> {
        if let [StrChunk::Lit(s)] = chunks.as_slice() {
            return Ok(Expr::String {
                value: s.clone(),
                span,
            });
        }
        let mut parts = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            match chunk {
                StrChunk::Lit(s) => parts.push(InterpPart::Literal(s)),
                StrChunk::Hole { src, offset } => {
                    // `offset` is relative to the source THIS parser lexed; `span_base`
                    // lifts it to an absolute position in the file (0 for a whole-file
                    // parse, the enclosing hole's position for a nested one).
                    parts.push(InterpPart::Hole(
                        self.parse_hole(&src, self.span_base + offset)?,
                    ));
                }
            }
        }
        Ok(Expr::Interpolation { parts, span })
    }

    /// Re-lex and parse one interpolation hole's source into a single expression, with the
    /// hole sitting at absolute byte position `abs` in the enclosing file. Every re-lexed
    /// token span is shifted to `abs` and stamped with this parser's file, and the
    /// sub-parser inherits `abs` as its `span_base` so any hole nested inside this one lifts
    /// to a true file position too. Keeping the `(file, offset)` oracle key accurate is what
    /// stops a hole's nodes from colliding with an unrelated node near the file start.
    pub(super) fn parse_hole(&self, src: &str, abs: usize) -> Result<Expr, ParseError> {
        let shift = |s: &Span| Span::in_file(s.start + abs as u32, s.end + abs as u32, self.file);
        let mut tokens = Lexer::tokenize_in_file(src, self.file).map_err(|e| ParseError {
            message: format!("in interpolation hole: {}", e.message),
            span: shift(&e.span),
        })?;
        for t in &mut tokens {
            t.span = shift(&t.span);
        }
        let mut parser = Parser::new(&tokens);
        parser.span_base = abs;
        let expr = parser.parse_expr()?;
        if !parser.is_at_end() {
            return Err(ParseError {
                message: "interpolation hole must be a single expression".to_string(),
                span: parser.current_span(),
            });
        }
        Ok(expr)
    }

    pub(super) fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.peek();

        match &token.kind {
            TokenKind::Number(n) => {
                let span = token.span.clone();
                let value = n.0;
                self.advance();
                Ok(Expr::Number { value, span })
            }
            TokenKind::String(chunks) => {
                let span = token.span.clone();
                let chunks = chunks.clone();
                self.advance();
                self.build_string_expr(chunks, span)
            }
            TokenKind::True => {
                let span = token.span.clone();
                self.advance();
                Ok(Expr::Bool { value: true, span })
            }
            TokenKind::False => {
                let span = token.span.clone();
                self.advance();
                Ok(Expr::Bool { value: false, span })
            }
            TokenKind::Unit => {
                // `$` is the unit value — sole inhabitant of the `$` (Unit) type.
                let span = token.span.clone();
                self.advance();
                Ok(Expr::Unit { span })
            }
            TokenKind::At => {
                // A leaf IO primitive reference: `@sleep`. `@` fuses with the following
                // identifier into the primitive's name (`@sleep`), so a call `@sleep(x)`
                // is an ordinary call of the ident `@sleep` (the postfix `(...)` is applied
                // by the caller). The name carries its `@` so codegen and the taint pass
                // recognize the deferring primitive by name.
                let at_span = token.span.clone();
                self.advance();
                let ident = self.peek();
                if ident.kind != TokenKind::Ident {
                    return Err(ParseError {
                        message: format!(
                            "Expected a primitive name after `@`, got {:?}",
                            ident.kind
                        ),
                        span: ident.span.clone(),
                    });
                }
                let name = format!("@{}", ident.text);
                let span = self.span(at_span.start, ident.span.end);
                self.advance();
                Ok(Expr::Ident { name, span })
            }
            TokenKind::Ident => {
                // A bare single-parameter lambda: `x => body` or `x :: Type => body`.
                // Detected before consuming the ident as a plain reference: an ident
                // followed by `=>` (or `:: Type =>`) is a function literal, not a value.
                if self.looks_like_bare_lambda() {
                    return self.parse_lambda_expr();
                }

                let span = token.span.clone();
                let name = token.text.clone();
                self.advance();

                // A same-line `{` makes this a record constructor: Ident { ... }. A
                // line-first `{` leaves the identifier a plain reference and the
                // `{ ... }` begins the next statement (same rule as `(` / `[`).
                if self.check_same_line(&TokenKind::BraceOpen) {
                    let start = span.start;
                    self.advance(); // consume '{'

                    let fields = self.parse_record_fields()?;
                    let span = self.span(start, self.previous_span().end);

                    Ok(Expr::Constructor {
                        type_name: name,
                        fields,
                        span,
                    })
                } else {
                    // Regular identifier - could be variable, function, or sum constructor
                    // The type checker will disambiguate
                    Ok(Expr::Ident { name, span })
                }
            }
            TokenKind::ParenOpen => {
                // A parenthesized parameter list introducing a lambda — `() => body`,
                // `(a, b) => body`, `(n :: Num) => body` — vs. an ordinary parenthesized
                // expression `(a + b)`. Distinguished by scanning to the matching `)`
                // and checking whether `=>` / `->` follows.
                if self.paren_starts_lambda() {
                    return self.parse_lambda_expr();
                }
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::ParenClose)?;
                Ok(expr)
            }
            // A pipe fence `[| … |]` opens a Map or Set literal; a plain `[` an array. The
            // empty set `[||]` lexes its two adjacent pipes as one `Or` token (maximal
            // munch), so a following `Or` also opens a fence — that case can only be the
            // empty set.
            TokenKind::BracketOpen
                if matches!(self.peek_ahead(1).kind, TokenKind::Pipe | TokenKind::Or) =>
            {
                self.parse_fenced_literal()
            }
            TokenKind::BracketOpen => self.parse_array(),
            TokenKind::BraceOpen => self.parse_record(),
            _ => Err(ParseError {
                message: format!("Unexpected token: {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
    }

    /// Parse a function-literal (lambda) expression: `params => body`, where `params` is
    /// `()`, a parenthesized list, or a single bare identifier — optionally with a `->`
    /// return type. The body is a single expression or a `< >` block. Shares its
    /// parameter grammar with `parse_function_decl`.
    pub(super) fn parse_lambda_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        let params = self.parse_param_list()?;

        let return_type = if self.check(&TokenKind::ReturnArrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(&TokenKind::Arrow)?;

        let body = if self.check(&TokenKind::BlockOpen) {
            self.parse_block()?
        } else {
            self.parse_expr()?
        };

        let span = self.span(start.start, self.previous_span().end);
        Ok(Expr::Lambda {
            params,
            return_type,
            body: Box::new(body),
            span,
        })
    }

    /// Parse a parameter list: `(a, b)`, `(a :: T, b :: T)`, `()`, or a single bare
    /// `name` / `name :: T` without parentheses. Stops before the `=>` / `->`.
    /// Shared by lambdas and (via the existing inline logic) function declarations.
    pub(super) fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.check(&TokenKind::ParenOpen) {
            self.advance();
            if !self.check(&TokenKind::ParenClose) {
                loop {
                    let param_name = self.expect_ident()?;
                    let param_type = if self.check(&TokenKind::TypeAnnotation) {
                        self.advance();
                        Some(self.parse_type()?)
                    } else {
                        None
                    };
                    params.push(Param {
                        name: param_name,
                        type_annotation: param_type,
                        span: self.previous_span(),
                    });
                    if !self.check(&TokenKind::Comma) {
                        break;
                    }
                    self.advance();
                }
            }
            self.expect(&TokenKind::ParenClose)?;
        } else if self.check(&TokenKind::Ident) {
            let param_name = self.expect_ident()?;
            let param_type = if self.check(&TokenKind::TypeAnnotation) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(Param {
                name: param_name,
                type_annotation: param_type,
                span: self.previous_span(),
            });
        }
        Ok(params)
    }

    /// The `{ … }` field list shared by an anonymous record literal and a named
    /// constructor: `name = value` entries, and `<-source` spread entries carrying the
    /// empty-string name (never a valid identifier) as their sentinel — consumers
    /// discriminate on `Expr::Spread`, not on the name. Assumes the opening brace is
    /// already consumed and consumes the closing one.
    pub(super) fn parse_record_fields(&mut self) -> Result<Vec<(String, Expr)>, ParseError> {
        let mut fields = Vec::new();

        if !self.check(&TokenKind::BraceClose) {
            loop {
                if let Some(spread) = self.try_parse_spread()? {
                    fields.push((String::new(), spread));
                } else {
                    let field_name = self.expect_ident()?;
                    self.expect(&TokenKind::Assign)?;
                    let value = self.parse_expr()?;
                    fields.push((field_name, value));
                }

                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::BraceClose)?;
        Ok(fields)
    }

    pub(super) fn parse_record(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BraceOpen)?;
        let fields = self.parse_record_fields()?;
        let span = self.span(start.start, self.previous_span().end);

        Ok(Expr::Record { fields, span })
    }

    pub(super) fn parse_array(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BracketOpen)?;

        let mut elements = Vec::new();

        if !self.check(&TokenKind::BracketClose) {
            loop {
                // An element beginning with `<-` is a SPREAD: `[<-xs, 4]` splices every
                // element of `xs` in, then appends `4`. Disambiguated from the infix
                // range `lo <- hi` by position — a leading `<-` is a spread (see
                // `try_parse_spread`), so `[1 <- 4]` is a one-element array holding the
                // range `[1,2,3,4]`, while `[<-xs, 4]` splices xs.
                if let Some(spread) = self.try_parse_spread()? {
                    elements.push(spread);
                } else {
                    elements.push(self.parse_expr()?);
                }
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::BracketClose)?;
        let span = self.span(start.start, self.previous_span().end);

        Ok(Expr::Array { elements, span })
    }

    /// Parse a pipe-fenced Map or Set literal, cursor at the opening `[` of a `[|`.
    ///   Map:  `[|k1 => v1, k2 => v2|]`   empty `[|=>|]`
    ///   Set:  `[|e1, e2|]`               empty `[||]`
    /// A first element followed by `=>` makes it a map; otherwise a set. The two empty
    /// forms are disambiguated by what follows `[|`: a `=>` is the empty map, an
    /// immediate closing `|]` (a `|`) is the empty set.
    pub(super) fn parse_fenced_literal(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BracketOpen)?;

        // Empty set `[||]`: the two adjacent pipes lexed as a single `Or` token.
        if self.check(&TokenKind::Or) {
            self.advance();
            self.expect(&TokenKind::BracketClose)?;
            let span = self.span(start.start, self.previous_span().end);
            return Ok(Expr::SetLiteral {
                elements: Vec::new(),
                span,
            });
        }

        self.expect(&TokenKind::Pipe)?;

        // Empty map `[|=>|]`.
        if self.check(&TokenKind::Arrow) {
            self.advance();
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            return Ok(Expr::MapLiteral {
                entries: Vec::new(),
                span,
            });
        }
        // Empty set `[||]` — the next `|` is the closing fence.
        if self.check(&TokenKind::Pipe) {
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            return Ok(Expr::SetLiteral {
                elements: Vec::new(),
                span,
            });
        }

        // Parse the first element as a KEY (lambda-suppressed): if a `=>` follows it is a
        // map, otherwise a set — and a set element, being a hashable value, is never a
        // lambda either, so the suppression is harmless there.
        let first = self.parse_fence_key()?;
        if self.check(&TokenKind::Arrow) {
            // Map: `first => value, ...`.
            self.advance();
            let value = self.parse_expr()?;
            let mut entries = vec![(first, value)];
            while self.check(&TokenKind::Comma) {
                self.advance();
                let key = self.parse_fence_key()?;
                self.expect(&TokenKind::Arrow)?;
                let value = self.parse_expr()?;
                entries.push((key, value));
            }
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            Ok(Expr::MapLiteral { entries, span })
        } else {
            // Set: `first, e2, ...`.
            let mut elements = vec![first];
            while self.check(&TokenKind::Comma) {
                self.advance();
                elements.push(self.parse_expr()?);
            }
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            Ok(Expr::SetLiteral { elements, span })
        }
    }

    /// Parse a map-literal key expression with lambda detection suppressed, so a key like
    /// `k` or `(a)` does not swallow the following `=>` "maps to" separator as a lambda.
    /// A key is always a hashable value, never a function, so this loses nothing.
    fn parse_fence_key(&mut self) -> Result<Expr, ParseError> {
        let prev = self.suppress_lambda;
        self.suppress_lambda = true;
        let result = self.parse_expr();
        self.suppress_lambda = prev;
        result
    }

    /// If the cursor is at a prefix `<-` (the FIRST token of an array element or record
    /// field), consume it and parse the spread source, returning `Expr::Spread`. Otherwise
    /// leave the cursor untouched and return `None` — the caller parses an ordinary
    /// element/field. This is the single point that decides spread-vs-range by position:
    /// a `<-` reached here begins an element, so it is a spread; a `<-` between two
    /// complete expressions is handled by `parse_range` as the infix range operator. The
    /// spread source is a full expression (so `[<-1 <- 4]` spreads the range `1 <- 4`).
    pub(super) fn try_parse_spread(&mut self) -> Result<Option<Expr>, ParseError> {
        if !self.check(&TokenKind::LeftArrow) {
            return Ok(None);
        }
        let start = self.current_span();
        self.advance(); // consume `<-`
        let source = self.parse_expr()?;
        let span = self.span(start.start, self.previous_span().end);
        Ok(Some(Expr::Spread {
            expr: Box::new(source),
            span,
        }))
    }
}
