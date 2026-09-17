// Lexer module for Quilon
// Handles tokenization of .qn source files

#[allow(clippy::module_inception)] // lexer::lexer holds the Lexer impl; layout is intentional
pub mod lexer;
pub mod token;

pub use lexer::Lexer;
pub use token::{
    FileId, ROOT_FILE, SYNTHESIZED_FILE, Span, StrChunk, Token, TokenKind, TokenLexError,
};
