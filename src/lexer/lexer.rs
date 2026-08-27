// Lexer implementation for Quilon

use crate::lexer::{FileId, ROOT_FILE, Span, Token, TokenKind, TokenLexError};
use logos::Logos;

/// Namespace for the lexer's entry point. Tokenizing is a single batch call
/// (`Lexer::tokenize`); there is no streaming/stateful lexer.
pub struct Lexer;

impl Lexer {
    /// Tokenize the entire root source and return all tokens. Root-only by definition:
    /// every span it produces claims [`ROOT_FILE`], so this is for the source the
    /// compiler was invoked on and nothing else. An imported module goes through
    /// [`Lexer::tokenize_in_file`] under its own id.
    pub fn tokenize(source: &str) -> Result<Vec<Token>, LexerError> {
        Self::tokenize_in_file(source, ROOT_FILE)
    }

    /// Tokenize `source` as the file identified by `file`, tagging every token's span
    /// with it. Offsets are relative to `source`, so only the pair `(file, offset)`
    /// identifies a position in a multi-module program.
    pub fn tokenize_in_file(source: &str, file: FileId) -> Result<Vec<Token>, LexerError> {
        let mut tokens = Vec::new();
        let mut lexer = TokenKind::lexer(source);

        loop {
            match lexer.next() {
                Some(Ok(kind)) if kind == TokenKind::Eof => {
                    let pos = source.len() as u32;
                    tokens.push(Token {
                        kind,
                        span: Span::in_file(pos, pos, file),
                        text: String::new(),
                        first_on_line: is_first_on_line(source, pos as usize),
                    });
                    break;
                }
                Some(Ok(kind)) => {
                    let span = lexer.span();
                    let text = source[span.clone()].to_string();
                    tokens.push(Token {
                        kind,
                        span: Span::in_file(span.start as u32, span.end as u32, file),
                        text,
                        first_on_line: is_first_on_line(source, span.start),
                    });
                }
                Some(Err(error)) => {
                    let span = lexer.span();
                    let text = source[span.clone()].to_string();
                    return Err(LexerError {
                        message: if error == TokenLexError::UnterminatedString {
                            "unterminated string literal".to_string()
                        } else {
                            format!("Invalid token: '{}'", text)
                        },
                        span: Span::in_file(span.start as u32, span.end as u32, file),
                    });
                }
                None => {
                    let pos = source.len() as u32;
                    tokens.push(Token {
                        kind: TokenKind::Eof,
                        span: Span::in_file(pos, pos, file),
                        text: String::new(),
                        first_on_line: is_first_on_line(source, pos as usize),
                    });
                    break;
                }
            }
        }

        reclassify_gt(&mut tokens);
        Ok(tokens)
    }
}

/// Turn each `>` that is really the greater-than operator into [`TokenKind::Gt`], leaving
/// the rest as the block close [`TokenKind::BlockClose`] — the rule stated on those two
/// token kinds.
///
/// One token of lookahead settles it because a block is not an operand of anything: within
/// a statement, a block close is never followed by the start of an operand, while a
/// comparison's `>` always is. The successor must also be on the same line — otherwise the
/// identifier opening the next statement would read as a comparison's right operand. What
/// stays unexpressible (as under any one-token rule) is a block close and a following
/// statement sharing a line: `< f = () => < 1 > y = 2 >` reads that `>` as a comparison.
/// No `>`'s reading depends on another's, so one flat pass suffices.
fn reclassify_gt(tokens: &mut [Token]) {
    let mut rest = tokens;
    while let [token, tail @ ..] = rest {
        if token.kind == TokenKind::BlockClose
            && tail
                .first()
                .is_some_and(|next| !next.first_on_line && next.kind.starts_operand())
        {
            token.kind = TokenKind::Gt;
        }
        rest = tail;
    }
}

/// Whether the position `at` in `source` is at the start of its line: only horizontal
/// whitespace (spaces/tabs) between it and the preceding newline or start of file. Exact
/// for token starts: everything the lexer skips is whitespace or a `~` comment, and a
/// comment always runs to end of line, so nothing but spaces/tabs can sit between a
/// line's newline and its first token.
/// Feeds `Token::first_on_line` (see `Parser::check_same_line` for the grammar rule).
fn is_first_on_line(source: &str, at: usize) -> bool {
    for b in source.as_bytes()[..at].iter().rev() {
        match b {
            b' ' | b'\t' => continue,
            b'\n' | b'\r' => return true,
            _ => return false,
        }
    }
    // Reached the start of the file over only horizontal whitespace: the file's first
    // token is the first on its line.
    true
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexerError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LexerError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds of every non-EOF token of `source`, for the `>` classification tests.
    fn kinds(source: &str) -> Vec<TokenKind> {
        Lexer::tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| *k != TokenKind::Eof)
            .collect()
    }

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
            TokenKind::String(chunks) => {
                assert_eq!(
                    chunks.as_slice(),
                    [crate::lexer::StrChunk::Lit("hello".into())]
                )
            }
            _ => panic!("Expected string"),
        }

        match &tokens[1].kind {
            TokenKind::String(chunks) => assert_eq!(
                chunks.as_slice(),
                [crate::lexer::StrChunk::Lit("world\n".into())]
            ),
            _ => panic!("Expected string with newline"),
        }
    }

    #[test]
    fn test_interpolated_string() {
        use crate::lexer::StrChunk;
        // `a`, hole `x + 1`, `!` — plus a doubled backtick collapsing to one literal.
        let tokens = Lexer::tokenize("\"a `x + 1`!``\"").unwrap();
        match &tokens[0].kind {
            TokenKind::String(chunks) => {
                assert_eq!(chunks.len(), 3);
                assert_eq!(chunks[0], StrChunk::Lit("a ".into()));
                match &chunks[1] {
                    StrChunk::Hole { src, .. } => assert_eq!(src, "x + 1"),
                    _ => panic!("Expected hole"),
                }
                assert_eq!(chunks[2], StrChunk::Lit("!`".into()));
            }
            _ => panic!("Expected interpolated string"),
        }
    }

    #[test]
    fn test_backtick_operator_token_outside_string() {
        // A bare backtick (defining the render operator) lexes as `Backtick`, never a hole.
        let tokens = Lexer::tokenize("` = () -> Text => \"x\"").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Backtick);
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
        // `<<?` is the test-only import — one token, not `<<` followed by a `?`.
        let test_only = Lexer::tokenize("<<? core.test.report").unwrap();
        assert_eq!(test_only[0].kind, TokenKind::TestImport);
        assert_eq!(test_only[1].kind, TokenKind::Ident);
    }

    #[test]
    fn test_delimiters() {
        // `>` here is followed by `{`, which starts an operand, so it is greater-than.
        let tokens = Lexer::tokenize("< > { } ( ) [ ]").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::BlockOpen);
        assert_eq!(tokens[1].kind, TokenKind::Gt);
        assert_eq!(tokens[2].kind, TokenKind::BraceOpen);
        assert_eq!(tokens[3].kind, TokenKind::BraceClose);
        assert_eq!(tokens[4].kind, TokenKind::ParenOpen);
        assert_eq!(tokens[5].kind, TokenKind::ParenClose);
    }

    #[test]
    fn test_block_close_is_the_default() {
        // Nothing that can start an operand follows the `>`, so every one of these
        // closes its block and produces no comparison.
        for source in [
            "<\n  x\n>",                             // end of line
            "< x >",                                 // end of file, no newline
            "f(() => <x>)",                          // before `)`
            "[() => <x>]",                           // before `]`
            "{ f = () => <x> }",                     // before `}`
            "f(() => <x>, 1)",                       // before `,`
            "<x> ~ trailing comment\ny",             // before a comment
            "<x>.size",                              // before `.`
            "<x> <= y",                              // before `<=`
            "<x> ? 1 : 2",                           // before `?`
            "<x> | y",                               // before `|`
            "<x> _",                                 // before `_`
            "describe(\"math\", () => < f(1, 1) >)", // one-line lambda argument
            "f(g(() => <x>))",                       // nested closers `>))`
        ] {
            let got = kinds(source);
            assert!(
                got.contains(&TokenKind::BlockClose) && !got.contains(&TokenKind::Gt),
                "`{source}` must close its block"
            );
        }

        // Two adjacent closers, which need the space: `>>` is the export marker.
        assert_eq!(
            kinds("< < x > >"),
            vec![
                TokenKind::BlockOpen,
                TokenKind::BlockOpen,
                TokenKind::Ident,
                TokenKind::BlockClose,
                TokenKind::BlockClose,
            ]
        );
    }

    #[test]
    fn test_greater_than_when_an_operand_follows() {
        // Every operand-starting token after `>` keeps it a comparison, and none of these
        // produces a block close.
        for source in [
            "a > b",
            "a > 1",
            "a > \"b\"",
            "\"b\" > \"a\"",
            "a > true",
            "a > false",
            "a > $",
            "a > @now",
            "a > (b + c)",
            "a > -b",
            "a > !flag",
            "a > [1]",
            "a > Point { x = 1 }",
            "f(x > y)",
            "[x > y, z]",
            "(a > b) == true",
            // A continuation line may lead with the operator.
            "a\n> b",
        ] {
            let got = kinds(source);
            assert!(
                got.contains(&TokenKind::Gt) && !got.contains(&TokenKind::BlockClose),
                "`{source}` must keep `>` as the comparison operator"
            );
        }

        // Chained comparisons: both `>` are operators.
        assert_eq!(
            kinds("a > b > c")
                .iter()
                .filter(|k| **k == TokenKind::Gt)
                .count(),
            2
        );

        // Comparisons inside a block, one of them its last expression: only the block's
        // own `>` closes.
        let inner = kinds("<\n  a > b\n  c > d\n>");
        assert_eq!(inner.iter().filter(|k| **k == TokenKind::Gt).count(), 2);
        assert_eq!(
            inner
                .iter()
                .filter(|k| **k == TokenKind::BlockClose)
                .count(),
            1
        );
    }

    #[test]
    fn test_neighbouring_tokens_are_unaffected() {
        // `>=`, `>>`, `->` and `=>` are their own tokens, untouched by the `>` rule.
        assert_eq!(kinds("a >= b")[1], TokenKind::Ge);
        assert_eq!(kinds("a >= 1")[1], TokenKind::Ge);
        assert_eq!(kinds(">> x = 1")[0], TokenKind::Export);
        assert_eq!(kinds(">>\nx")[0], TokenKind::Export);
        assert_eq!(kinds("() -> Num => 1")[2], TokenKind::ReturnArrow);
        assert_eq!(kinds("() -> Num => 1")[4], TokenKind::Arrow);
        // `<` never reclassifies: it is always `BlockOpen`, less-than being the parser's job.
        assert_eq!(kinds("a < b")[1], TokenKind::BlockOpen);
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
        assert_eq!(tokens[2].kind, TokenKind::BlockOpen); // `<` is always block-open
        // `>` here is followed by `<=`, which cannot start an operand, so it closes.
        assert_eq!(tokens[3].kind, TokenKind::BlockClose);
        assert_eq!(tokens[4].kind, TokenKind::Le);
        assert_eq!(tokens[5].kind, TokenKind::Ge);
    }

    #[test]
    fn test_comments() {
        let tokens = Lexer::tokenize("x ~ this is a comment\ny").unwrap();
        assert_eq!(tokens.len(), 3); // x, y, EOF (comment skipped)
        assert_eq!(tokens[0].text, "x");
        assert_eq!(tokens[1].text, "y");
        // The comment is transparent to line tracking: `y` opens ITS line (the
        // comment's line ended at the newline).
        assert!(tokens[1].first_on_line);
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
    fn test_first_on_line_tracking() {
        // `first_on_line` marks the first token of each source line.
        let tokens = Lexer::tokenize("a = f()\n(1 + 2)").unwrap();
        let firsts: Vec<(&str, bool)> = tokens
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| (t.text.as_str(), t.first_on_line))
            .collect();
        assert_eq!(
            firsts,
            vec![
                ("a", true),
                ("=", false),
                ("f", false),
                ("(", false),
                (")", false),
                ("(", true), // opens line 2
                ("1", false),
                ("+", false),
                ("2", false),
                (")", false),
            ]
        );

        // Indentation doesn't matter — the first NON-BLANK token opens the line.
        let indented = Lexer::tokenize("x\n    [1]").unwrap();
        assert!(
            indented[1].first_on_line,
            "indented `[` still opens its line"
        );
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
