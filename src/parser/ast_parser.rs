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
        self.parse_primary()
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
            _ => Err(ParseError {
                message: format!("Unexpected token: {:?}", token.kind),
                span: token.span.clone(),
            }),
        }
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
}
