// Lexer implementation for Quilon

use crate::lexer::{Span, Token, TokenKind};
use logos::Logos;

/// Namespace for the lexer's entry point. Tokenizing is a single batch call
/// (`Lexer::tokenize`); there is no streaming/stateful lexer.
pub struct Lexer;

impl Lexer {
    /// Tokenize the entire source and return all tokens
    pub fn tokenize(source: &str) -> Result<Vec<Token>, LexerError> {
        let mut tokens = Vec::new();
        let mut lexer = TokenKind::lexer(source);

        loop {
            match lexer.next() {
                Some(Ok(kind)) if kind == TokenKind::Eof => {
                    let pos = source.len();
                    tokens.push(Token::new(kind, Span::new(pos, pos), String::new()));
                    break;
                }
                Some(Ok(kind)) => {
                    let span = lexer.span();
                    let text = source[span.clone()].to_string();
                    tokens.push(Token::new(kind, Span::new(span.start, span.end), text));
                }
                Some(Err(_)) => {
                    let span = lexer.span();
                    let text = source[span.clone()].to_string();
                    return Err(LexerError {
                        message: format!("Invalid token: '{}'", text),
                        span: Span::new(span.start, span.end),
                    });
                }
                None => {
                    let pos = source.len();
                    tokens.push(Token::new(
                        TokenKind::Eof,
                        Span::new(pos, pos),
                        String::new(),
                    ));
                    break;
                }
            }
        }

        Ok(tokens)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexerError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}", self.message, self.span)
    }
}

impl std::error::Error for LexerError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)] // 3.14 is a generic decimal test value, not PI
    fn test_numbers() {
        let tokens = Lexer::tokenize("42 3.14 0.5").unwrap();
        assert_eq!(tokens.len(), 4); // 3 numbers + EOF

        match &tokens[0].kind {
            TokenKind::Number(n) => assert_eq!(n.0, 42.0),
            _ => panic!("Expected number"),
        }

        match &tokens[1].kind {
            TokenKind::Number(n) => assert_eq!(n.0, 3.14),
            _ => panic!("Expected number"),
        }
    }

    #[test]
    fn test_strings() {
        let tokens = Lexer::tokenize(r#""hello" "world\n""#).unwrap();
        assert_eq!(tokens.len(), 3); // 2 strings + EOF

        match &tokens[0].kind {
            TokenKind::String(s) => assert_eq!(s, "hello"),
            _ => panic!("Expected string"),
        }

        match &tokens[1].kind {
            TokenKind::String(s) => assert_eq!(s, "world\n"),
            _ => panic!("Expected string with newline"),
        }
    }

    #[test]
    fn test_booleans() {
        let tokens = Lexer::tokenize("true false").unwrap();
        assert_eq!(tokens.len(), 3); // 2 bools + EOF
        assert_eq!(tokens[0].kind, TokenKind::True);
        assert_eq!(tokens[1].kind, TokenKind::False);
    }

    #[test]
    fn test_identifiers() {
        let tokens = Lexer::tokenize("name user_id _temp").unwrap();
        assert_eq!(tokens.len(), 4); // 3 idents + EOF
        assert_eq!(tokens[0].kind, TokenKind::Ident);
        assert_eq!(tokens[0].text, "name");
        assert_eq!(tokens[1].text, "user_id");
    }

    #[test]
    fn test_operators() {
        let tokens = Lexer::tokenize("= => -> :: |> ? |").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Assign);
        assert_eq!(tokens[1].kind, TokenKind::Arrow);
        assert_eq!(tokens[2].kind, TokenKind::ReturnArrow);
        assert_eq!(tokens[3].kind, TokenKind::TypeAnnotation);
        assert_eq!(tokens[4].kind, TokenKind::Pipeline);
        assert_eq!(tokens[5].kind, TokenKind::Question);
        assert_eq!(tokens[6].kind, TokenKind::Pipe);
    }

    #[test]
    fn test_module_and_entry_symbols() {
        // `<<` import, `^` entry point, `>>` export
        let tokens = Lexer::tokenize("<< ^ >>").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Import);
        assert_eq!(tokens[1].kind, TokenKind::EntryPoint);
        assert_eq!(tokens[2].kind, TokenKind::Export);
        // `<<` must lex as a single Import token, not two BlockOpen
        let two = Lexer::tokenize("< <").unwrap();
        assert_eq!(two[0].kind, TokenKind::BlockOpen);
        assert_eq!(two[1].kind, TokenKind::BlockOpen);
    }

    #[test]
    fn test_delimiters() {
        let tokens = Lexer::tokenize("< > { } ( ) [ ]").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::BlockOpen);
        assert_eq!(tokens[1].kind, TokenKind::BlockClose);
        assert_eq!(tokens[2].kind, TokenKind::BraceOpen);
        assert_eq!(tokens[3].kind, TokenKind::BraceClose);
        assert_eq!(tokens[4].kind, TokenKind::ParenOpen);
        assert_eq!(tokens[5].kind, TokenKind::ParenClose);
    }

    #[test]
    fn test_arithmetic() {
        let tokens = Lexer::tokenize("+ - * / %").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Plus);
        assert_eq!(tokens[1].kind, TokenKind::Minus);
        assert_eq!(tokens[2].kind, TokenKind::Star);
        assert_eq!(tokens[3].kind, TokenKind::Slash);
        assert_eq!(tokens[4].kind, TokenKind::Percent);
    }

    #[test]
    fn test_comparison() {
        let tokens = Lexer::tokenize("== != < > <= >=").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Eq);
        assert_eq!(tokens[1].kind, TokenKind::Ne);
        assert_eq!(tokens[2].kind, TokenKind::BlockOpen); // < is block open
        assert_eq!(tokens[3].kind, TokenKind::BlockClose); // > is block close
        assert_eq!(tokens[4].kind, TokenKind::Le);
        assert_eq!(tokens[5].kind, TokenKind::Ge);
    }

    #[test]
    fn test_comments() {
        let tokens = Lexer::tokenize("x ~ this is a comment\ny").unwrap();
        assert_eq!(tokens.len(), 3); // x, y, EOF (comment skipped)
        assert_eq!(tokens[0].text, "x");
        assert_eq!(tokens[1].text, "y");
    }

    #[test]
    fn test_simple_function() {
        let source = "add = (a :: Num, b :: Num) => a + b";
        let tokens = Lexer::tokenize(source).unwrap();

        assert_eq!(tokens[0].text, "add");
        assert_eq!(tokens[1].kind, TokenKind::Assign);
        assert_eq!(tokens[2].kind, TokenKind::ParenOpen);
        assert_eq!(tokens[3].text, "a");
        assert_eq!(tokens[4].kind, TokenKind::TypeAnnotation);
    }

    #[test]
    fn test_pipeline() {
        let source = "data |> filter .active |> collect";
        let tokens = Lexer::tokenize(source).unwrap();

        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipeline));
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Pipeline)
                .count(),
            2
        );
    }

    #[test]
    fn test_block() {
        let source = "process = data => <\n  data |> map transform\n>";
        let tokens = Lexer::tokenize(source).unwrap();

        assert!(tokens.iter().any(|t| t.kind == TokenKind::BlockOpen));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::BlockClose));
    }

    #[test]
    fn test_position_tracking() {
        let tokens = Lexer::tokenize("abc def").unwrap();
        assert_eq!(tokens[0].span.start, 0);
        assert_eq!(tokens[0].span.end, 3);
        assert_eq!(tokens[1].span.start, 4);
        assert_eq!(tokens[1].span.end, 7);
    }
}
