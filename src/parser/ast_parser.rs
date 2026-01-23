// Parser implementation - simple recursive descent

use crate::ast::{Expr, BinOp, UnaryOp, VarDecl, Item, Program, Param, FunctionDecl};
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
        // For now, assume all items are variable declarations
        self.parse_var_decl().map(Item::VarDecl)
    }

    fn parse_var_decl(&mut self) -> Result<VarDecl, ParseError> {
        let start = self.current_span();

        // Check for 'mut'
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };

        // Get identifier
        let name = self.expect_ident()?;

        // Optional type annotation
        let type_annotation = if self.check(&TokenKind::TypeAnnotation) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        // Expect '='
        self.expect(&TokenKind::Assign)?;

        // Parse value expression
        let value = self.parse_expr()?;

        let end = self.previous_span();

        Ok(VarDecl {
            mutable,
            name,
            type_annotation,
            value,
            span: Span::new(start.start, end.end),
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_logical_or()?;

        // Check for ternary operator: expr ? then : else
        if self.check(&TokenKind::Question) {
            self.advance();
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

        Ok(expr)
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
            let right = self.parse_additive()?;
            let span = Span::new(left.span().start, right.span().end);
            left = Expr::Pipeline {
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
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
                let span = Span::new(expr.span().start, self.previous_span().end);
                expr = Expr::FieldAccess {
                    expr: Box::new(expr),
                    field,
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
            _ => Err(ParseError {
                message: format!("Unexpected token: {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
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
}
