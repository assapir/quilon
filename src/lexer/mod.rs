// Lexer module for Quilon
// Handles tokenization of .ql source files

#[allow(clippy::module_inception)] // lexer::lexer holds the Lexer impl; layout is intentional
pub mod lexer;
pub mod token;

pub use lexer::Lexer;
pub use token::{Span, Token, TokenKind};
