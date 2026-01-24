// Parser implementation - simple recursive descent

use crate::ast::{Expr, BinOp, UnaryOp, VarDecl, Item, Program, Param, FunctionDecl, ForPattern, TypeDecl, TypeDef, MethodDecl};
use crate::lexer::{Token, TokenKind, Span};

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}", self.message, self.span)
    }
}

impl std::error::Error for ParseError {}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(tokens: &'a [Token]) -> Result<Program, ParseError> {
        let mut parser = Self::new(tokens);
        parser.parse_program()
    }

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut items = Vec::new();

        while !self.is_at_end() {
            items.push(self.parse_item()?);
        }

        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        // Three possibilities:
        // 1. Type declaration: Name = { fields and methods }
        // 2. Function declaration: name = params => body
        // 3. Variable declaration: name = value
        
        let start = self.current_span();
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };

        let name = self.expect_ident()?;

        // Check for type annotation
        let type_annotation = if self.check(&TokenKind::TypeAnnotation) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(&TokenKind::Assign)?;

        // Check if it's a type declaration (Name = { ... })
        // Type declarations can't be mutable and don't have type annotations
        if !mutable && type_annotation.is_none() && self.check(&TokenKind::BraceOpen) {
            // This is a type declaration
            return self.parse_type_decl(name, start);
        }

        // Check if it's a function:
        // - name = => ...  (no params)
        // - name = (params) => ...
        // - name = param => ...  (single param, no parens)
        // Need to be careful not to confuse with: result = (2 + 3) * 4
        
        let is_function = if self.check(&TokenKind::Arrow) {
            true
        } else if self.check(&TokenKind::ParenOpen) {
            // Look ahead to see if this is parameter list or expression
            // Parameter list ends with ) =>
            // We need to scan ahead to find matching )
            let mut depth = 1;
            let mut idx = 1;
            let mut found_arrow = false;
            
            while idx < 50 && depth > 0 {  // reasonable limit for lookahead
                let ahead = self.peek_ahead(idx);
                match ahead.kind {
                    TokenKind::ParenOpen => depth += 1,
                    TokenKind::ParenClose => {
                        depth -= 1;
                        if depth == 0 {
                            // Check if next token after ) is => or ->
                            let next = self.peek_ahead(idx + 1);
                            found_arrow = next.kind == TokenKind::Arrow || next.kind == TokenKind::ReturnArrow;
                        }
                    }
                    TokenKind::Eof => break,
                    _ => {}
                }
                idx += 1;
            }
            found_arrow
        } else if self.check(&TokenKind::Ident) {
            // Single param without parens: check if followed by => or ::
            let ahead = self.peek_ahead(1);
            ahead.kind == TokenKind::Arrow || ahead.kind == TokenKind::TypeAnnotation
        } else {
            false
        };

        if is_function {
            self.parse_function_decl(name, start, type_annotation)
        } else {
            let value = self.parse_expr()?;
            let end = self.previous_span();
            
            Ok(Item::VarDecl(VarDecl {
                mutable,
                name,
                type_annotation,
                value,
                span: Span::new(start.start, end.end),
            }))
        }
    }

    fn parse_function_decl(
        &mut self,
        name: String,
        start: Span,
        return_type: Option<crate::ast::Type>,
    ) -> Result<Item, ParseError> {
        let mut params = Vec::new();

        // Parse parameters: (a, b) or (a :: Type, b :: Type) or single param or just =>
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
            // Single parameter without parentheses
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

        // Optional return type annotation with ->
        let return_type = if self.check(&TokenKind::ReturnArrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            return_type
        };

        // Expect =>
        self.expect(&TokenKind::Arrow)?;

        // Parse body (can be a block or single expression)
        let body = if self.check(&TokenKind::BlockOpen) {
            self.parse_block()?
        } else {
            self.parse_expr()?
        };

        let end = self.previous_span();

        Ok(Item::FunctionDecl(FunctionDecl {
            name,
            params,
            return_type,
            body,
            span: Span::new(start.start, end.end),
        }))
    }

    fn parse_type_decl(&mut self, name: String, start: Span) -> Result<Item, ParseError> {
        // Parse type definition: Name = { field :: Type, ... method = => body, ... }
        self.expect(&TokenKind::BraceOpen)?;
        
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        
        while !self.check(&TokenKind::BraceClose) && !self.is_at_end() {
            let field_name = self.expect_ident()?;
            
            if self.check(&TokenKind::TypeAnnotation) {
                // This is a field: name :: Type
                self.advance();
                let field_type = self.parse_type()?;
                fields.push((field_name, field_type));
            } else if self.check(&TokenKind::Assign) {
                // This is a method: name = params => body
                self.advance();
                
                let method_start = self.current_span();
                let mut params = Vec::new();
                
                // Parse method parameters (note: "it" is implicit, not included here)
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
                    // Single parameter without parentheses
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
                
                // Optional return type annotation
                let return_type = if self.check(&TokenKind::ReturnArrow) {
                    self.advance();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                
                // Expect =>
                self.expect(&TokenKind::Arrow)?;
                
                // Parse method body
                let body = if self.check(&TokenKind::BlockOpen) {
                    self.parse_block()?
                } else {
                    self.parse_expr()?
                };
                
                let method_end = self.previous_span();
                
                methods.push(MethodDecl {
                    name: field_name,
                    params,
                    return_type,
                    body,
                    span: Span::new(method_start.start, method_end.end),
                });
            } else {
                return Err(ParseError {
                    message: format!("Expected :: or = after field/method name"),
                    span: self.peek().span.clone(),
                });
            }
            
            // Optional comma separator
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
        }
        
        self.expect(&TokenKind::BraceClose)?;
        let end = self.previous_span();
        
        Ok(Item::TypeDecl(TypeDecl {
            name,
            type_def: TypeDef::Record { fields, methods },
            span: Span::new(start.start, end.end),
        }))
    }

    fn parse_block(&mut self) -> Result<Expr, ParseError> {
        use crate::ast::Statement;
        
        let start = self.current_span();
        self.expect(&TokenKind::BlockOpen)?;

        let mut stmts = Vec::new();

        while !self.check(&TokenKind::BlockClose) && !self.is_at_end() {
            // Try to parse as item first (for nested declarations)
            if self.check(&TokenKind::Mut) || 
               (self.check(&TokenKind::Ident) && self.peek_ahead(1).kind == TokenKind::Assign) {
                // This looks like a declaration
                let item = self.parse_item()?;
                stmts.push(Statement::Item(item));
            } else {
                stmts.push(Statement::Expr(self.parse_expr()?));
            }

            // Expressions in blocks can be separated by newlines (already skipped by lexer)
            // or we just continue to the next one
        }

        self.expect(&TokenKind::BlockClose)?;
        let span = Span::new(start.start, self.previous_span().end);

        Ok(Expr::Block { stmts, span })
    }

    fn peek_ahead(&self, offset: usize) -> &Token {
        let pos = self.pos + offset;
        if pos < self.tokens.len() {
            &self.tokens[pos]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
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
                let span = Span::new(expr.span().start, else_expr.span().end);

                return Ok(Expr::If {
                    cond: Box::new(expr),
                    then: Box::new(then_expr),
                    else_: Box::new(else_expr),
                    span,
                });
            }
        }

        Ok(expr)
    }

    fn parse_match(&mut self, expr: Expr) -> Result<Expr, ParseError> {
        let start = expr.span().start;
        let mut arms = Vec::new();

        // Parse match arms: | pattern => body
        while self.check(&TokenKind::Pipe) {
            self.advance();
            
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::Arrow)?;
            let body = self.parse_expr()?;
            let arm_span = Span::new(pattern.span().start, body.span().end);
            
            arms.push(crate::ast::MatchArm {
                pattern,
                body,
                span: arm_span,
            });
        }

        if arms.is_empty() {
            return Err(ParseError {
                message: "Match expression must have at least one arm".to_string(),
                span: Span::new(start, start),
            });
        }

        let end = arms.last().unwrap().span.end;

        Ok(Expr::Match {
            expr: Box::new(expr),
            arms,
            span: Span::new(start, end),
        })
    }

    fn parse_pattern(&mut self) -> Result<crate::ast::Pattern, ParseError> {
        use crate::ast::Pattern;
        
        let token = self.peek();
        
        match &token.kind {
            TokenKind::Ident => {
                let name = token.text.clone();
                let span = token.span.clone();
                self.advance();
                
                // Check if it's a constructor: Name(patterns) or Name pattern
                if self.check(&TokenKind::ParenOpen) {
                    self.advance();
                    let mut args = Vec::new();
                    
                    if !self.check(&TokenKind::ParenClose) {
                        loop {
                            args.push(self.parse_pattern()?);
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
                        args,
                        span: Span::new(span.start, end),
                    })
                } else {
                    // Just an identifier pattern
                    Ok(Pattern::Ident { name, span })
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

    fn parse_logical_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_logical_and()?;

        while self.check(&TokenKind::Or) {
            self.advance();
            let right = self.parse_logical_and()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::Or,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;

        while self.check(&TokenKind::And) {
            self.advance();
            let right = self.parse_equality()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::And,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;

        while let Some(op) = self.match_equality() {
            let right = self.parse_comparison()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_pipeline()?;

        while let Some(op) = self.match_comparison() {
            let right = self.parse_pipeline()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_pipeline(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive()?;

        while self.check(&TokenKind::Pipeline) {
            self.advance();
            
            // Check if this is a for loop: |> for pattern => body
            if self.check(&TokenKind::For) {
                self.advance(); // consume 'for'
                
                // Parse pattern: either `item` or `(item, index)`
                let pattern = if self.check(&TokenKind::ParenOpen) {
                    // Parse (item, index) pattern
                    self.advance(); // consume '('
                    
                    let item = self.expect_ident()?;
                    self.expect(&TokenKind::Comma)?;
                    let index = self.expect_ident()?;
                    
                    self.expect(&TokenKind::ParenClose)?;
                    let end = self.previous_span();
                    
                    ForPattern::ItemIndex {
                        item,
                        index,
                        span: Span::new(left.span().start, end.end),
                    }
                } else {
                    // Parse simple item pattern
                    let item = self.expect_ident()?;
                    let span = self.previous_span();
                    
                    ForPattern::Item {
                        name: item,
                        span,
                    }
                };
                
                // Expect =>
                self.expect(&TokenKind::Arrow)?;
                
                // Parse body expression
                let body = self.parse_expr()?;
                let span = Span::new(left.span().start, body.span().end);
                
                left = Expr::ForLoop {
                    collection: Box::new(left),
                    pattern,
                    body: Box::new(body),
                    span,
                };
            } else {
                let right = self.parse_additive()?;
                let span = Span::new(left.span().start, right.span().end);
                left = Expr::Pipeline {
                    left: Box::new(left),
                    right: Box::new(right),
                    span,
                };
            }
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative()?;

        while let Some(op) = self.match_additive() {
            let right = self.parse_multiplicative()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        while let Some(op) = self.match_multiplicative() {
            let right = self.parse_unary()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::Minus) {
            let start = self.current_span();
            self.advance();
            let expr = self.parse_unary()?;
            let span = Span::new(start.start, expr.span().end);
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                span,
            });
        }

        if self.check(&TokenKind::Not) {
            let start = self.current_span();
            self.advance();
            let expr = self.parse_unary()?;
            let span = Span::new(start.start, expr.span().end);
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            if self.check(&TokenKind::Dot) {
                self.advance();
                let field = self.expect_ident()?;
                
                // Check if this is a method call: obj.method(args)
                if self.check(&TokenKind::ParenOpen) {
                    // Method call: desugar obj.method(a, b) to method(obj, a, b)
                    self.advance(); // consume '('
                    
                    // Parse arguments
                    let mut args = vec![expr]; // receiver is first argument
                    
                    if !self.check(&TokenKind::ParenClose) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.check(&TokenKind::Comma) {
                                break;
                            }
                            self.advance();
                        }
                    }
                    
                    self.expect(&TokenKind::ParenClose)?;
                    let span = Span::new(args[0].span().start, self.previous_span().end);
                    
                    // Create function call with method name
                    expr = Expr::Call {
                        func: Box::new(Expr::Ident {
                            name: field,
                            span: span.clone(),
                        }),
                        args,
                        span,
                    };
                } else {
                    // Regular field access
                    let span = Span::new(expr.span().start, self.previous_span().end);
                    expr = Expr::FieldAccess {
                        expr: Box::new(expr),
                        field,
                        span,
                    };
                }
            } else if self.check(&TokenKind::BracketOpen) {
                // Array indexing
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::BracketClose)?;
                let span = Span::new(expr.span().start, self.previous_span().end);
                expr = Expr::Index {
                    expr: Box::new(expr),
                    index: Box::new(index),
                    span,
                };
            } else if self.check(&TokenKind::ParenOpen) {
                // Function call
                self.advance();
                let mut args = Vec::new();

                if !self.check(&TokenKind::ParenClose) {
                    loop {
                        args.push(self.parse_expr()?);
                        if !self.check(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }

                self.expect(&TokenKind::ParenClose)?;
                let span = Span::new(expr.span().start, self.previous_span().end);
                expr = Expr::Call {
                    func: Box::new(expr),
                    args,
                    span,
                };
            } else {
                break;
            }
        }

        Ok(expr)
    }

    // Match helper functions
    fn match_equality(&mut self) -> Option<BinOp> {
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

    fn match_comparison(&mut self) -> Option<BinOp> {
        match &self.peek().kind {
            TokenKind::Le => {
                self.advance();
                Some(BinOp::Le)
            }
            TokenKind::Ge => {
                self.advance();
                Some(BinOp::Ge)
            }
            _ => None,
        }
    }

    fn match_additive(&mut self) -> Option<BinOp> {
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

    fn match_multiplicative(&mut self) -> Option<BinOp> {
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
            _ => None,
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.peek();

        match &token.kind {
            TokenKind::Number(n) => {
                let span = token.span.clone();
                let value = n.0;
                self.advance();
                Ok(Expr::Number { value, span })
            }
            TokenKind::String(s) => {
                let span = token.span.clone();
                let value = s.clone();
                self.advance();
                Ok(Expr::String { value, span })
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
            TokenKind::Ident => {
                let span = token.span.clone();
                let name = token.text.clone();
                self.advance();
                Ok(Expr::Ident { name, span })
            }
            TokenKind::ParenOpen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::ParenClose)?;
                Ok(expr)
            }
            TokenKind::BracketOpen => {
                self.parse_array()
            }
            TokenKind::BraceOpen => {
                self.parse_record()
            }
            _ => Err(ParseError {
                message: format!("Unexpected token: {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
    }

    fn parse_record(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BraceOpen)?;

        let mut fields = Vec::new();

        if !self.check(&TokenKind::BraceClose) {
            loop {
                let field_name = self.expect_ident()?;
                self.expect(&TokenKind::Assign)?;
                let value = self.parse_expr()?;
                fields.push((field_name, value));
                
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::BraceClose)?;
        let span = Span::new(start.start, self.previous_span().end);

        Ok(Expr::Record { fields, span })
    }

    fn parse_array(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::BracketOpen)?;

        let mut elements = Vec::new();

        if !self.check(&TokenKind::BracketClose) {
            loop {
                elements.push(self.parse_expr()?);
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::BracketClose)?;
        let span = Span::new(start.start, self.previous_span().end);

        Ok(Expr::Array { elements, span })
    }

    fn parse_type(&mut self) -> Result<crate::ast::Type, ParseError> {
        let token = self.peek();

        match token.text.as_str() {
            "Num" => {
                self.advance();
                Ok(crate::ast::Type::Num)
            }
            "String" => {
                self.advance();
                Ok(crate::ast::Type::String)
            }
            "Bool" => {
                self.advance();
                Ok(crate::ast::Type::Bool)
            }
            _ => Err(ParseError {
                message: format!("Expected type, got {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
    }

    // Helper methods

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
            // Allow >> as a special function name (entry point)
            self.advance();
            Ok(">>".to_string())
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
            Span::new(0, 0)
        }
    }
}

/// Parse a Quilon program from tokens
pub fn parse(tokens: &[Token]) -> Result<Program, ParseError> {
    Parser::parse(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    #[test]
    fn test_parse_number() {
        let tokens = Lexer::tokenize("x = 42").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());

        let program = result.unwrap();
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn test_parse_string() {
        let tokens = Lexer::tokenize(r#"msg = "hello""#).unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_boolean() {
        let tokens = Lexer::tokenize("flag = true").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_mutable() {
        let tokens = Lexer::tokenize("mut counter = 0").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());

        let program = result.unwrap();
        if let Item::VarDecl(decl) = &program.items[0] {
            assert!(decl.mutable);
            assert_eq!(decl.name, "counter");
        } else {
            panic!("Expected VarDecl");
        }
    }

    #[test]
    fn test_parse_with_type() {
        let tokens = Lexer::tokenize("x :: Num = 42").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_arithmetic() {
        let tokens = Lexer::tokenize("result = 2 + 3 * 4").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());

        let program = result.unwrap();
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn test_parse_comparison() {
        let tokens = Lexer::tokenize("flag = x >= 5").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_logical() {
        let tokens = Lexer::tokenize("result = a && b || c").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_unary() {
        let tokens = Lexer::tokenize("neg = -x").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());

        let tokens2 = Lexer::tokenize("not_flag = !flag").unwrap();
        let result2 = parse(&tokens2);
        assert!(result2.is_ok());
    }

    #[test]
    fn test_parse_function_call() {
        let tokens = Lexer::tokenize("result = add(1, 2)").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_field_access() {
        let tokens = Lexer::tokenize("name = user.name").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_pipeline() {
        let tokens = Lexer::tokenize("result = data |> filter |> collect").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_ternary() {
        let tokens = Lexer::tokenize("abs = x >= 0 ? x : -x").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_pattern_match() {
        let tokens = Lexer::tokenize("result = value ? | Some(x) => x | None => 0").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::VarDecl(decl) = &program.items[0] {
            if let Expr::Match { arms, .. } = &decl.value {
                assert_eq!(arms.len(), 2);
            } else {
                panic!("Expected Match expression");
            }
        } else {
            panic!("Expected VarDecl");
        }
    }

    #[test]
    fn test_parse_pattern_wildcard() {
        let tokens = Lexer::tokenize("result = value ? | 0 => \"zero\" | _ => \"other\"").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_record() {
        let tokens = Lexer::tokenize("user = { name = \"Alice\", age = 30 }").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::VarDecl(decl) = &program.items[0] {
            if let Expr::Record { fields, .. } = &decl.value {
                assert_eq!(fields.len(), 2);
            } else {
                panic!("Expected Record expression");
            }
        } else {
            panic!("Expected VarDecl");
        }
    }

    #[test]
    fn test_parse_empty_record() {
        let tokens = Lexer::tokenize("empty = {}").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_parentheses() {
        let tokens = Lexer::tokenize("result = (2 + 3) * 4").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_array() {
        let tokens = Lexer::tokenize("nums = [1, 2, 3, 4, 5]").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_chained_calls() {
        let tokens = Lexer::tokenize("result = obj.method(arg).field").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }

    #[test]
    fn test_precedence() {
        // Should parse as: 2 + (3 * 4)
        let tokens = Lexer::tokenize("x = 2 + 3 * 4").unwrap();
        let program = parse(&tokens).unwrap();

        if let Item::VarDecl(decl) = &program.items[0] {
            // The root should be BinOp(Add)
            if let Expr::BinOp { op: BinOp::Add, .. } = &decl.value {
                // Correct precedence
            } else {
                panic!("Expected Add at root, got {:?}", decl.value);
            }
        }
    }
    
    #[test]
    fn test_parse_simple_function() {
        let tokens = Lexer::tokenize("add = (a, b) => a + b").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            assert_eq!(func.name, "add");
            assert_eq!(func.params.len(), 2);
            assert_eq!(func.params[0].name, "a");
            assert_eq!(func.params[1].name, "b");
        } else {
            panic!("Expected function declaration");
        }
    }
    
    #[test]
    fn test_parse_function_with_types() {
        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            assert_eq!(func.params.len(), 2);
            assert!(func.params[0].type_annotation.is_some());
            assert!(func.return_type.is_some());
        } else {
            panic!("Expected function declaration");
        }
    }
    
    #[test]
    fn test_parse_no_param_function() {
        let tokens = Lexer::tokenize("main = => 42").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_parse_block() {
        let tokens = Lexer::tokenize("test = => < x = 1 y = 2 >").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            if let Expr::Block { stmts, .. } = &func.body {
                assert_eq!(stmts.len(), 2);
            } else {
                panic!("Expected block expression");
            }
        }
    }
    
    #[test]
    fn test_parse_function_with_block() {
        let tokens = Lexer::tokenize("greet = name => < msg = \"Hello\" msg >").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_parse_for_loop_simple() {
        let tokens = Lexer::tokenize("test = => [1, 2, 3] |> for n => print(n)").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            if let Expr::ForLoop { pattern, .. } = &func.body {
                match pattern {
                    crate::ast::ForPattern::Item { name, .. } => {
                        assert_eq!(name, "n");
                    }
                    _ => panic!("Expected simple item pattern"),
                }
            } else {
                panic!("Expected for loop expression");
            }
        }
    }
    
    #[test]
    fn test_parse_for_loop_with_index() {
        let tokens = Lexer::tokenize("test = => items |> for (val, i) => print(val)").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            if let Expr::ForLoop { pattern, .. } = &func.body {
                match pattern {
                    crate::ast::ForPattern::ItemIndex { item, index, .. } => {
                        assert_eq!(item, "val");
                        assert_eq!(index, "i");
                    }
                    _ => panic!("Expected item-index pattern"),
                }
            } else {
                panic!("Expected for loop expression");
            }
        }
    }
    
    #[test]
    fn test_parse_for_loop_with_block_body() {
        let tokens = Lexer::tokenize("test = => [1, 2, 3] |> for n => n * 2").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::FunctionDecl(func) = &program.items[0] {
            if let Expr::ForLoop { body, .. } = &func.body {
                // Success - we have a for loop body
                let _ = body;
            } else {
                panic!("Expected for loop expression");
            }
        }
    }
    
    #[test]
    fn test_parse_method_call() {
        let tokens = Lexer::tokenize("result = user.getName()").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::VarDecl(var) = &program.items[0] {
            // Should be desugared to a Call with Ident("getName") as func
            if let Expr::Call { func, args, .. } = &var.value {
                // func should be Ident("getName")
                if let Expr::Ident { name, .. } = func.as_ref() {
                    assert_eq!(name, "getName");
                    // First arg should be the receiver (user)
                    assert_eq!(args.len(), 1);
                    if let Expr::Ident { name, .. } = &args[0] {
                        assert_eq!(name, "user");
                    } else {
                        panic!("Expected receiver as first argument");
                    }
                } else {
                    panic!("Expected Ident as function in method call");
                }
            } else {
                panic!("Expected method call to be desugared to Call");
            }
        } else {
            panic!("Expected variable declaration");
        }
    }
    
    #[test]
    fn test_parse_method_call_with_args() {
        let tokens = Lexer::tokenize("result = user.setAge(25)").unwrap();
        let result = parse(&tokens);
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::VarDecl(var) = &program.items[0] {
            if let Expr::Call { func, args, .. } = &var.value {
                if let Expr::Ident { name, .. } = func.as_ref() {
                    assert_eq!(name, "setAge");
                    // Should have 2 args: receiver and the argument
                    assert_eq!(args.len(), 2);
                }
            }
        }
    }
    
    #[test]
    fn test_parse_chained_method_calls() {
        let tokens = Lexer::tokenize("result = user.getName().toUpper()").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_parse_type_decl_with_fields() {
        let tokens = Lexer::tokenize("User = { name :: String, age :: Num }").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::TypeDecl(decl) = &program.items[0] {
            assert_eq!(decl.name, "User");
            if let TypeDef::Record { fields, methods } = &decl.type_def {
                assert_eq!(fields.len(), 2);
                assert_eq!(methods.len(), 0);
            } else {
                panic!("Expected Record type definition");
            }
        } else {
            panic!("Expected type declaration");
        }
    }
    
    #[test]
    fn test_parse_type_decl_with_methods() {
        let tokens = Lexer::tokenize("User = { 
  name :: String, 
  getName = => it.name 
}").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::TypeDecl(decl) = &program.items[0] {
            assert_eq!(decl.name, "User");
            if let TypeDef::Record { fields, methods } = &decl.type_def {
                assert_eq!(fields.len(), 1);
                assert_eq!(methods.len(), 1);
                assert_eq!(methods[0].name, "getName");
                assert_eq!(methods[0].params.len(), 0); // "it" is implicit
            } else {
                panic!("Expected Record type definition");
            }
        } else {
            panic!("Expected type declaration");
        }
    }
    
    #[test]
    fn test_parse_type_decl_method_with_params() {
        let tokens = Lexer::tokenize("User = { 
  age :: Num,
  incrementAge = amount => it.age + amount
}").unwrap();
        let result = parse(&tokens);
        if result.is_err() {
            eprintln!("Error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
        
        let program = result.unwrap();
        if let Item::TypeDecl(decl) = &program.items[0] {
            if let TypeDef::Record { fields: _, methods } = &decl.type_def {
                assert_eq!(methods[0].name, "incrementAge");
                assert_eq!(methods[0].params.len(), 1);
                assert_eq!(methods[0].params[0].name, "amount");
            }
        }
    }
}
