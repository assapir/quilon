//! The expression grammar: the precedence chain from assignment down to postfix and
//! primary, plus the literal forms (lambda, record, array, interpolated string).
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_expression(&mut self) -> Result<Expression, ParseError> {
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
    pub(super) fn parse_assignment(&mut self) -> Result<Expression, ParseError> {
        let expression = self.parse_ternary()?;

        if self.check(&TokenKind::MutAssign) && matches!(expression, Expression::FieldAccess { .. })
        {
            self.advance(); // consume `:=`
            // Depth-guard the value: a `:=` chain (`a.x := b.y := …`) re-enters
            // `parse_assignment` directly, bypassing the `parse_expression` funnel.
            let value = self.nested(Self::parse_assignment)?;
            let span = self.span(expression.span().start, value.span().end);
            return Ok(Expression::FieldAssign {
                target: Box::new(expression),
                value: Box::new(value),
                span,
            });
        }

        Ok(expression)
    }

    pub(super) fn parse_ternary(&mut self) -> Result<Expression, ParseError> {
        let expression = self.parse_logical_or()?;

        // Check for ? operator - could be ternary or pattern match
        if self.check(&TokenKind::Question) {
            self.advance();

            // Check if it's pattern match (next token is |) or ternary
            if self.check(&TokenKind::Pipe) {
                // Pattern match: expression ? | pattern => body | pattern => body
                return self.parse_match(expression);
            } else {
                // Ternary: expression ? then : else
                let then_expression = self.parse_expression()?;
                self.expect(&TokenKind::Colon)?;
                let else_expression = self.parse_expression()?;
                let span = self.span(expression.span().start, else_expression.span().end);

                return Ok(Expression::If {
                    condition: Box::new(expression),
                    then: Box::new(then_expression),
                    else_: Box::new(else_expression),
                    span,
                });
            }
        }

        Ok(expression)
    }

    pub(super) fn parse_logical_or(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_logical_and()?;

        while self.check(&TokenKind::Or) {
            self.advance();
            let right = self.parse_logical_and()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator: BinaryOperator::Or,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_logical_and(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_equality()?;

        while self.check(&TokenKind::And) {
            self.advance();
            let right = self.parse_equality()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator: BinaryOperator::And,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_equality(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_comparison()?;

        while let Some(operator) = self.match_equality() {
            let right = self.parse_comparison()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_comparison(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_range()?;

        while let Some(operator) = self.match_comparison() {
            let right = self.parse_range()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    /// Infix range `lo <- hi` → inclusive `[]Num` (see the `Expression::Range` node).
    /// Non-associative: consumes at most one `<-`, so `a <- b <- c` is rejected.
    pub(super) fn parse_range(&mut self) -> Result<Expression, ParseError> {
        let left = self.parse_additive()?;

        if self.check(&TokenKind::LeftArrow) {
            self.advance(); // consume `<-`
            let right = self.parse_additive()?;
            let span = self.span(left.span().start, right.span().end);
            return Ok(Expression::Range {
                start: Box::new(left),
                end: Box::new(right),
                span,
            });
        }

        Ok(left)
    }

    pub(super) fn parse_additive(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_multiplicative()?;

        while let Some(operator) = self.match_additive() {
            let right = self.parse_multiplicative()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_multiplicative(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_unary()?;

        while let Some(operator) = self.match_multiplicative() {
            let right = self.parse_unary()?;
            let span = self.span(left.span().start, right.span().end);
            left = Expression::BinaryOperator {
                left: Box::new(left),
                operator,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expression, ParseError> {
        if self.check(&TokenKind::Minus) {
            let start = self.current_span();
            self.advance();
            // Depth-guard the operand: a chain of prefix operators (`---…x`,
            // `!!!…x`) re-enters `parse_unary` without passing through `parse_expression`,
            // so bound it here too to keep deep chains from overflowing the stack.
            let expression = self.nested(Self::parse_unary)?;
            let span = self.span(start.start, expression.span().end);
            return Ok(Expression::UnaryOperator {
                operator: UnaryOperator::Neg,
                expression: Box::new(expression),
                span,
            });
        }

        if self.check(&TokenKind::Not) {
            let start = self.current_span();
            self.advance();
            let expression = self.nested(Self::parse_unary)?; // depth-guarded (see Neg branch)
            let span = self.span(start.start, expression.span().end);
            return Ok(Expression::UnaryOperator {
                operator: UnaryOperator::Not,
                expression: Box::new(expression),
                span,
            });
        }

        self.parse_postfix()
    }

    /// Parse postfix continuations: `.field` / `.method(args)` / `[index]` / `(args)`.
    /// The `(` / `[` continuations are gated by `check_same_line` — a line-first `(`
    /// or `[` begins a new statement instead (a `.`-led line still chains). The record
    /// constructor `Ident { ... }` in `parse_primary` is gated by the same predicate.
    pub(super) fn parse_postfix(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.parse_primary()?;

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
                    let mut arguments = vec![expression]; // receiver is first argument

                    if !self.check(&TokenKind::ParenClose) {
                        loop {
                            arguments.push(self.parse_expression()?);
                            if !self.check(&TokenKind::Comma) {
                                break;
                            }
                            self.advance();
                        }
                    }

                    self.expect(&TokenKind::ParenClose)?;
                    let span = self.span(arguments[0].span().start, self.previous_span().end);

                    // Create function call with method name
                    expression = Expression::Call {
                        function: Box::new(Expression::Identifier {
                            name: field,
                            span: span.clone(),
                        }),
                        arguments,
                        member_call: true,
                        span,
                    };
                } else {
                    // Regular field access
                    let span = self.span(expression.span().start, self.previous_span().end);
                    expression = Expression::FieldAccess {
                        expression: Box::new(expression),
                        field,
                        span,
                    };
                }
            } else if self.check_same_line(&TokenKind::BracketOpen) {
                // Array indexing
                self.advance();
                let index = self.parse_expression()?;
                self.expect(&TokenKind::BracketClose)?;
                let span = self.span(expression.span().start, self.previous_span().end);
                expression = Expression::Index {
                    expression: Box::new(expression),
                    index: Box::new(index),
                    span,
                };
            } else if self.check_same_line(&TokenKind::ParenOpen) {
                // Function call
                self.advance();
                let mut arguments = Vec::new();

                if !self.check(&TokenKind::ParenClose) {
                    loop {
                        arguments.push(self.parse_expression()?);
                        if !self.check(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }

                self.expect(&TokenKind::ParenClose)?;
                let span = self.span(expression.span().start, self.previous_span().end);
                expression = Expression::Call {
                    function: Box::new(expression),
                    arguments,
                    member_call: false,
                    span,
                };
            } else {
                break;
            }
        }

        Ok(expression)
    }

    // Match helper functions
    pub(super) fn match_equality(&mut self) -> Option<BinaryOperator> {
        // A trailing `== =`/`!= =` is an operator MEMBER definition (e.g. inside a type's
        // `{ }` block), not `expr == …` — stop so `parse_member_block` picks it up. The
        // other operator levels guard the same way.
        if self.at_operator_definition() {
            return None;
        }
        if self.check(&TokenKind::Eq) {
            self.advance();
            Some(BinaryOperator::Eq)
        } else if self.check(&TokenKind::Ne) {
            self.advance();
            Some(BinaryOperator::Ne)
        } else {
            None
        }
    }

    pub(super) fn match_comparison(&mut self) -> Option<BinaryOperator> {
        if self.at_operator_definition() {
            return None;
        }
        match &self.peek().kind {
            TokenKind::Le => {
                self.advance();
                Some(BinaryOperator::Le)
            }
            TokenKind::Ge => {
                self.advance();
                Some(BinaryOperator::Ge)
            }
            // `<` doubles as the block-open delimiter, but in comparison position
            // (after a complete left operand) a block can never start, so a bare
            // `<` here is unambiguously the less-than operator.
            TokenKind::BlockOpen => {
                self.advance();
                Some(BinaryOperator::Lt)
            }
            // The lexer already distinguished a greater-than `>` (token `Gt`) from a
            // block-closing `>` (token `BlockClose`), so a `Gt` here is unambiguously
            // the operator.
            TokenKind::Gt => {
                self.advance();
                Some(BinaryOperator::Gt)
            }
            _ => None,
        }
    }

    pub(super) fn match_additive(&mut self) -> Option<BinaryOperator> {
        if self.at_operator_definition() {
            return None;
        }
        if self.check(&TokenKind::Plus) {
            self.advance();
            Some(BinaryOperator::Add)
        } else if self.check(&TokenKind::Minus) {
            self.advance();
            Some(BinaryOperator::Sub)
        } else {
            None
        }
    }

    pub(super) fn match_multiplicative(&mut self) -> Option<BinaryOperator> {
        if self.at_operator_definition() {
            return None;
        }
        match &self.peek().kind {
            TokenKind::Star => {
                self.advance();
                Some(BinaryOperator::Mul)
            }
            TokenKind::Slash => {
                self.advance();
                Some(BinaryOperator::Div)
            }
            TokenKind::Percent => {
                self.advance();
                Some(BinaryOperator::Mod)
            }
            // `+-` / `-+` — set intersection (symmetric); binds tighter than `+`/`-`
            // union/difference, mirroring arithmetic's product-over-sum precedence.
            TokenKind::PlusMinus | TokenKind::MinusPlus => {
                self.advance();
                Some(BinaryOperator::SetIntersect)
            }
            _ => None,
        }
    }

    /// Build a string expression from the lexer's chunks. A single literal chunk is a
    /// plain `Expression::String`; any interpolation hole yields an `Expression::Interpolation` whose
    /// holes are re-lexed and parsed as expressions.
    pub(super) fn build_string_expression(
        &self,
        chunks: Vec<StrChunk>,
        span: Span,
    ) -> Result<Expression, ParseError> {
        if let [StrChunk::Lit(s)] = chunks.as_slice() {
            return Ok(Expression::String {
                value: s.clone(),
                span,
            });
        }
        let mut parts = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            match chunk {
                StrChunk::Lit(s) => parts.push(InterpolationPart::Literal(s)),
                StrChunk::Hole { src, offset } => {
                    // `offset` is relative to the source THIS parser lexed; `span_base`
                    // lifts it to an absolute position in the file (0 for a whole-file
                    // parse, the enclosing hole's position for a nested one).
                    parts.push(InterpolationPart::Hole(
                        self.parse_hole(&src, self.span_base + offset)?,
                    ));
                }
            }
        }
        Ok(Expression::Interpolation { parts, span })
    }

    /// Re-lex and parse one interpolation hole's source into a single expression, with the
    /// hole sitting at absolute byte position `abs` in the enclosing file. Every re-lexed
    /// token span is shifted to `abs` and stamped with this parser's file, and the
    /// sub-parser inherits `abs` as its `span_base` so any hole nested inside this one lifts
    /// to a true file position too. Keeping the `(file, offset)` oracle key accurate is what
    /// stops a hole's nodes from colliding with an unrelated node near the file start.
    pub(super) fn parse_hole(&self, src: &str, abs: usize) -> Result<Expression, ParseError> {
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
        // A hole may spell qualified names, so it sees the same import bindings the
        // enclosing file has accumulated so far.
        parser.module_paths = self.module_paths.clone();
        let expression = parser.parse_expression()?;
        if !parser.is_at_end() {
            return Err(ParseError {
                message: "interpolation hole must be a single expression".to_string(),
                span: parser.current_span(),
            });
        }
        Ok(expression)
    }

    pub(super) fn parse_primary(&mut self) -> Result<Expression, ParseError> {
        let token = self.peek();

        match &token.kind {
            TokenKind::Number(n) => {
                let span = token.span.clone();
                let value = n.0;
                self.advance();
                Ok(Expression::Number { value, span })
            }
            TokenKind::String(chunks) => {
                let span = token.span.clone();
                let chunks = chunks.clone();
                self.advance();
                self.build_string_expression(chunks, span)
            }
            TokenKind::True => {
                let span = token.span.clone();
                self.advance();
                Ok(Expression::Bool { value: true, span })
            }
            TokenKind::False => {
                let span = token.span.clone();
                self.advance();
                Ok(Expression::Bool { value: false, span })
            }
            TokenKind::Unit => {
                // `$` is the unit value — sole inhabitant of the `$` (Unit) type.
                let span = token.span.clone();
                self.advance();
                Ok(Expression::Unit { span })
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
                Ok(Expression::Identifier { name, span })
            }
            TokenKind::Ident => {
                // A bare single-parameter lambda: `x => body` or `x :: Type => body`.
                // Detected before consuming the ident as a plain reference: an ident
                // followed by `=>` (or `:: Type =>`) is a function literal, not a value.
                if self.looks_like_bare_lambda() {
                    return self.parse_lambda_expression();
                }

                // A qualified reference through an imported module binding — `http.send`,
                // `http.Request { … }`, `core.test.describe` — reads as ONE dotted name;
                // the postfix loop then applies `(args)` / `{ fields }` / further `.`s to
                // it like any other reference.
                let (name, span, _) = self.parse_name_or_qualified();

                // A same-line `{` makes this a record constructor: Ident { ... }. A
                // line-first `{` leaves the identifier a plain reference and the
                // `{ ... }` begins the next statement (same rule as `(` / `[`).
                if self.check_same_line(&TokenKind::BraceOpen) {
                    let start = span.start;
                    self.advance(); // consume '{'

                    let fields = self.parse_record_fields()?;
                    let span = self.span(start, self.previous_span().end);

                    Ok(Expression::Constructor {
                        type_name: name,
                        fields,
                        span,
                    })
                } else {
                    // Regular identifier - could be variable, function, or sum constructor
                    // The type checker will disambiguate
                    Ok(Expression::Identifier { name, span })
                }
            }
            TokenKind::ParenOpen => {
                // A parenthesized parameter list introducing a lambda — `() => body`,
                // `(a, b) => body`, `(n :: Num) => body` — vs. an ordinary parenthesized
                // expression `(a + b)`. Distinguished by scanning to the matching `)`
                // and checking whether `=>` / `->` follows.
                if self.paren_starts_lambda() {
                    return self.parse_lambda_expression();
                }
                self.advance();
                let expression = self.parse_expression()?;
                self.expect(&TokenKind::ParenClose)?;
                Ok(expression)
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
            _ => {
                // The lexer's `>` rule reads a `>` as greater-than exactly when an
                // operand follows, so `TokenKind::starts_operand` must accept nothing
                // the operand grammar rejects — a divergence would silently turn a
                // comparison into a block close. The prefix operators are exempt here:
                // `parse_unary` consumes them before this function is ever reached.
                debug_assert!(
                    !token.kind.starts_operand()
                        || matches!(token.kind, TokenKind::Minus | TokenKind::Not),
                    "starts_operand accepts {:?}, which the operand grammar rejects",
                    token.kind
                );
                Err(ParseError {
                    message: format!("Unexpected token: {:?}", token.kind),
                    span: token.span.clone(),
                })
            }
        }
    }

    /// Parse a function-literal (lambda) expression: `parameters => body`, where `parameters` is
    /// `()`, a parenthesized list, or a single bare identifier — optionally with a `->`
    /// return type. The body is a single expression or a `< >` block. Shares its
    /// parameter grammar with `parse_function_declaration`.
    pub(super) fn parse_lambda_expression(&mut self) -> Result<Expression, ParseError> {
        let start = self.current_span();
        let parameters = self.parse_parameter_list()?;

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
            self.parse_expression()?
        };

        let span = self.span(start.start, self.previous_span().end);
        Ok(Expression::Lambda {
            parameters,
            return_type,
            body: Box::new(body),
            span,
        })
    }

    /// Parse a parameter list: `(a, b)`, `(a :: T, b :: T)`, `()`, or a single bare
    /// `name` / `name :: T` without parentheses. Stops before the `=>` / `->`.
    /// Shared by lambdas, methods and function declarations — so this is also where the
    /// `MAX_PARAMETERS` rule is enforced, once, for every parameter list a program writes.
    pub(super) fn parse_parameter_list(&mut self) -> Result<Vec<Parameter>, ParseError> {
        let mut parameters = Vec::new();
        if self.check(&TokenKind::ParenOpen) {
            self.advance();
            if !self.check(&TokenKind::ParenClose) {
                loop {
                    if parameters.len() == MAX_PARAMETERS {
                        return Err(ParseError {
                            message: format!(
                                "a function takes at most {MAX_PARAMETERS} parameters — group \
                                 them into a record type and take that record as one parameter \
                                 instead"
                            ),
                            span: self.current_span(),
                        });
                    }
                    let parameter_name = self.expect_ident()?;
                    let parameter_type = if self.check(&TokenKind::TypeAnnotation) {
                        self.advance();
                        Some(self.parse_type()?)
                    } else {
                        None
                    };
                    parameters.push(Parameter {
                        name: parameter_name,
                        type_annotation: parameter_type,
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
            let parameter_name = self.expect_ident()?;
            let parameter_type = if self.check(&TokenKind::TypeAnnotation) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            parameters.push(Parameter {
                name: parameter_name,
                type_annotation: parameter_type,
                span: self.previous_span(),
            });
        }
        Ok(parameters)
    }

    /// The `{ … }` field list shared by an anonymous record literal and a named
    /// constructor: `name = value` entries, and `<-source` spread entries carrying the
    /// empty-string name (never a valid identifier) as their sentinel — consumers
    /// discriminate on `Expression::Spread`, not on the name. Assumes the opening brace is
    /// already consumed and consumes the closing one.
    pub(super) fn parse_record_fields(&mut self) -> Result<Vec<(String, Expression)>, ParseError> {
        let mut fields = Vec::new();

        if !self.check(&TokenKind::BraceClose) {
            loop {
                if let Some(spread) = self.try_parse_spread()? {
                    fields.push((String::new(), spread));
                } else {
                    let field_name = self.expect_ident()?;
                    self.expect(&TokenKind::Assign)?;
                    let value = self.parse_expression()?;
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

    pub(super) fn parse_record(&mut self) -> Result<Expression, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BraceOpen)?;
        let fields = self.parse_record_fields()?;
        let span = self.span(start.start, self.previous_span().end);

        Ok(Expression::Record { fields, span })
    }

    pub(super) fn parse_array(&mut self) -> Result<Expression, ParseError> {
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
                    elements.push(self.parse_expression()?);
                }
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::BracketClose)?;
        let span = self.span(start.start, self.previous_span().end);

        Ok(Expression::Array { elements, span })
    }

    /// Parse a pipe-fenced Map or Set literal, cursor at the opening `[` of a `[|`.
    ///   Map:  `[|k1 => v1, k2 => v2|]`   empty `[|=>|]`
    ///   Set:  `[|e1, e2|]`               empty `[||]`
    /// A first element followed by `=>` makes it a map; otherwise a set. The two empty
    /// forms are disambiguated by what follows `[|`: a `=>` is the empty map, an
    /// immediate closing `|]` (a `|`) is the empty set.
    pub(super) fn parse_fenced_literal(&mut self) -> Result<Expression, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BracketOpen)?;

        // Empty set `[||]`: the two adjacent pipes lexed as a single `Or` token.
        if self.check(&TokenKind::Or) {
            self.advance();
            self.expect(&TokenKind::BracketClose)?;
            let span = self.span(start.start, self.previous_span().end);
            return Ok(Expression::SetLiteral {
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
            return Ok(Expression::MapLiteral {
                entries: Vec::new(),
                span,
            });
        }
        // Empty set `[||]` — the next `|` is the closing fence.
        if self.check(&TokenKind::Pipe) {
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            return Ok(Expression::SetLiteral {
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
            let value = self.parse_expression()?;
            let mut entries = vec![(first, value)];
            while self.check(&TokenKind::Comma) {
                self.advance();
                let key = self.parse_fence_key()?;
                self.expect(&TokenKind::Arrow)?;
                let value = self.parse_expression()?;
                entries.push((key, value));
            }
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            Ok(Expression::MapLiteral { entries, span })
        } else {
            // Set: `first, e2, ...`.
            let mut elements = vec![first];
            while self.check(&TokenKind::Comma) {
                self.advance();
                elements.push(self.parse_expression()?);
            }
            self.expect_fence_close()?;
            let span = self.span(start.start, self.previous_span().end);
            Ok(Expression::SetLiteral { elements, span })
        }
    }

    /// Parse a map-literal key expression with lambda detection suppressed, so a key like
    /// `k` or `(a)` does not swallow the following `=>` "maps to" separator as a lambda.
    /// A key is always a hashable value, never a function, so this loses nothing.
    fn parse_fence_key(&mut self) -> Result<Expression, ParseError> {
        let prev = self.suppress_lambda;
        self.suppress_lambda = true;
        let result = self.parse_expression();
        self.suppress_lambda = prev;
        result
    }

    /// If the cursor is at a prefix `<-` (the FIRST token of an array element or record
    /// field), consume it and parse the spread source, returning `Expression::Spread`. Otherwise
    /// leave the cursor untouched and return `None` — the caller parses an ordinary
    /// element/field. This is the single point that decides spread-vs-range by position:
    /// a `<-` reached here begins an element, so it is a spread; a `<-` between two
    /// complete expressions is handled by `parse_range` as the infix range operator. The
    /// spread source is a full expression (so `[<-1 <- 4]` spreads the range `1 <- 4`).
    pub(super) fn try_parse_spread(&mut self) -> Result<Option<Expression>, ParseError> {
        if !self.check(&TokenKind::LeftArrow) {
            return Ok(None);
        }
        let start = self.current_span();
        self.advance(); // consume `<-`
        let source = self.parse_expression()?;
        let span = self.span(start.start, self.previous_span().end);
        Ok(Some(Expression::Spread {
            expression: Box::new(source),
            span,
        }))
    }
}
