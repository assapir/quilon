// Integration tests for Quilon lexer

use quilon::ast::{Expression, Item, Statement};
use quilon::lexer::{Lexer, ROOT_FILE, TokenKind};
use quilon::parser::parse;

#[test]
fn test_hello_world() {
    let source = r#"
main = => <
  print "Hello, World!"
>
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    // Should have: main, =, =>, <, print, string, >, EOF
    assert!(tokens.len() >= 7);
    assert_eq!(tokens[0].text, "main");
    assert_eq!(tokens[1].kind, TokenKind::Assign);
    assert_eq!(tokens[2].kind, TokenKind::Arrow);
}

#[test]
fn test_factorial() {
    let source = r#"
factorial = n :: Num => <
  n ?
    | 0 => 1
    | n => n * factorial (n - 1)
>
"#;

    let tokens = Lexer::tokenize(source).unwrap();
    let result = Lexer::tokenize(source);
    assert!(result.is_ok());

    // Check for key tokens
    assert!(tokens.iter().any(|t| t.text == "factorial"));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::TypeAnnotation));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Question));
    assert!(tokens.iter().filter(|t| t.kind == TokenKind::Pipe).count() >= 2);
}

#[test]
fn test_mutable_variable() {
    // `:=` is the mutable bind/reassign operator (replaces the old `mut` keyword).
    let source = "counter := 0";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].text, "counter");
    assert_eq!(tokens[1].kind, TokenKind::MutAssign);
    assert!(matches!(tokens[2].kind, TokenKind::Number(_)));
}

#[test]
fn test_function_with_parameters() {
    let source = "add = (a :: Num, b :: Num) -> Num => < a + b >";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::ReturnArrow));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Arrow));
    // Two type annotations: a :: Num and b :: Num
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::TypeAnnotation)
            .count(),
        2
    );
}

/// `\e` is the ESC byte — the lead-in of an ANSI sequence, and the one control character
/// a `.qn` source cannot otherwise write (a raw ESC in a file is invisible).
#[test]
fn test_escape_e_is_the_esc_byte() {
    let tokens = Lexer::tokenize(r#""\e[1;31mred\e[0m""#).unwrap();
    match &tokens[0].kind {
        TokenKind::String(chunks) => match chunks.as_slice() {
            [quilon::lexer::StrChunk::Lit(s)] => {
                assert_eq!(s, "\u{1b}[1;31mred\u{1b}[0m");
            }
            other => panic!("expected a single literal chunk, got {other:?}"),
        },
        other => panic!("expected a string token, got {other:?}"),
    }
}

#[test]
fn test_string_escapes() {
    let source = r#""hello\nworld\t\"\\""#;
    let tokens = Lexer::tokenize(source).unwrap();

    match &tokens[0].kind {
        TokenKind::String(chunks) => {
            let s = match chunks.as_slice() {
                [quilon::lexer::StrChunk::Lit(s)] => s,
                _ => panic!("Expected a single literal chunk"),
            };
            assert!(s.contains('\n'));
            assert!(s.contains('\t'));
            assert!(s.contains('"'));
            assert!(s.contains('\\'));
        }
        _ => panic!("Expected string token"),
    }
}

#[test]
fn test_unterminated_string_stops_at_raw_newline() {
    let source = "value = 1\n\"unterminated\nnext = 2";
    let error = Lexer::tokenize(source).expect_err("raw newline must end a string");
    let opening_quote = source.find('"').unwrap();

    assert_eq!(error.message, "unterminated string literal");
    assert_eq!(error.span.start, opening_quote as u32);
    assert_eq!(error.span.end, opening_quote as u32 + 1);
    assert_eq!(error.span.file, ROOT_FILE);
    assert_eq!(quilon::lexer::Span::line_col(source, opening_quote), (2, 1));
}

#[test]
fn test_interpolation_still_works_with_string_newline_guard() {
    let tokens = Lexer::tokenize("\"hello `name`\"").unwrap();

    match &tokens[0].kind {
        TokenKind::String(chunks) => {
            assert_eq!(chunks[0], quilon::lexer::StrChunk::Lit("hello ".into()));
            assert_eq!(
                chunks[1],
                quilon::lexer::StrChunk::Hole {
                    src: "name".into(),
                    offset: 8,
                }
            );
        }
        other => panic!("expected interpolated string, got {other:?}"),
    }
}

#[test]
fn test_invalid_string_escape_keeps_invalid_token_error() {
    let error = Lexer::tokenize(r#""bad\q""#).expect_err("invalid escape must fail");

    assert!(error.message.starts_with("Invalid token:"));

    let error = Lexer::tokenize("\"bad\\q\rnext").expect_err("invalid escape must fail");
    assert!(error.message.starts_with("Invalid token:"));
}

#[test]
fn test_backslash_before_raw_newline_is_unterminated_string() {
    for source in ["\"bad\\\nnext", "\"bad\\\rnext"] {
        let error = Lexer::tokenize(source).expect_err("raw newline after backslash must fail");
        assert_eq!(error.message, "unterminated string literal");
    }
}

#[test]
fn test_nested_interpolation_backslash_before_newline_is_unterminated_string() {
    let source = "\"outer `f(\\\"bad\\\n\\\")`\"";
    let error = Lexer::tokenize(source).expect_err("nested raw newline must fail");

    assert_eq!(error.message, "unterminated string literal");
}

#[test]
fn test_all_comparison_operators() {
    let source = "a == b && c != d && e <= f && g >= h";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::Eq));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Ne));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Le));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Ge));
    assert_eq!(
        tokens.iter().filter(|t| t.kind == TokenKind::And).count(),
        3
    );
}

#[test]
fn test_nested_blocks() {
    let source = r#"
outer = => <
  inner = => <
    print "nested"
  >
>
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    let open_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BlockOpen)
        .count();
    let close_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BlockClose)
        .count();

    assert_eq!(open_count, 2);
    assert_eq!(close_count, 2);
}

#[test]
fn test_array_syntax() {
    let source = "[1, 2, 3, 4, 5]";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].kind, TokenKind::BracketOpen);
    assert_eq!(
        tokens.iter().filter(|t| t.kind == TokenKind::Comma).count(),
        4
    );
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BracketClose));
}

#[test]
fn test_record_syntax() {
    let source = "{ name :: Text, age :: Num }";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].kind, TokenKind::BraceOpen);
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BraceClose));
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::TypeAnnotation)
            .count(),
        2
    );
}

#[test]
fn test_generic_type() {
    let source = "Result{T, E}";
    let tokens = Lexer::tokenize(source).unwrap();

    assert_eq!(tokens[0].text, "Result");
    assert_eq!(tokens[1].kind, TokenKind::BraceOpen);
    // Result, {, T, ,, E, }, EOF -> index 5
    assert_eq!(tokens[5].kind, TokenKind::BraceClose);
}

#[test]
fn test_multiline_comment() {
    let source = r#"
x = 1
~ This is a comment
~ Another comment
y = 2
"#;

    let tokens = Lexer::tokenize(source).unwrap();

    // Should have: x, =, 1, y, =, 2, EOF (comments skipped)
    assert!(tokens.iter().any(|t| t.text == "x"));
    assert!(tokens.iter().any(|t| t.text == "y"));
}

#[test]
fn test_ternary_operator() {
    let source = "result = x > 0 ? x : -x";
    let tokens = Lexer::tokenize(source).unwrap();

    assert!(tokens.iter().any(|t| t.kind == TokenKind::Question));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Colon));
}

#[test]
fn test_spans_carry_the_file_they_came_from() {
    // `tokenize` is the root source; a module is tokenized under its own id. Offsets
    // are per-file and restart at 0, so identical ranges in two files are only told
    // apart by the id — that is what keeps per-expression types from colliding.
    let source = "x = 1";
    let root = Lexer::tokenize(source).unwrap();
    let module = Lexer::tokenize_in_file(source, 3).unwrap();

    assert!(root.iter().all(|t| t.span.file == ROOT_FILE));
    assert!(module.iter().all(|t| t.span.file == 3));
    assert_eq!(root[0].span.start, module[0].span.start);
    assert_eq!(root[0].span.end, module[0].span.end);
    assert_ne!(root[0].span, module[0].span);
}

/// Trojan Source guard (CVE-2021-42574 class, see `quilon::lexer::bidi`): a balanced
/// isolate inside a string literal lexes normally, and the literal text is preserved
/// exactly (no reordering, no stripped or injected characters).
#[test]
fn test_balanced_bidi_isolate_in_a_literal_lexes() {
    let source = "x = \"\u{2067}hello\u{2069}\"";
    let tokens = Lexer::tokenize(source).expect("a balanced isolate must lex");
    match &tokens[2].kind {
        TokenKind::String(chunks) => assert_eq!(
            chunks.as_slice(),
            [quilon::lexer::StrChunk::Lit(
                "\u{2067}hello\u{2069}".to_string()
            )]
        ),
        other => panic!("expected a string token, got {other:?}"),
    }
}

/// Nesting an embedding inside an isolate — RLI, then LRE, then PDF (closes the
/// embedding), then PDI (closes the isolate) — is valid UAX #9 nesting and must lex.
#[test]
fn test_embedding_nested_inside_isolate_in_a_literal_lexes() {
    let source = "\"\u{2067}\u{202A}x\u{202C}\u{2069}\"";
    assert!(Lexer::tokenize(source).is_ok());
}

/// An opener with no matching closer before the string ends is a lex error naming the
/// character and the token it was found in.
#[test]
fn test_unterminated_bidi_override_in_a_literal_errors() {
    let source = "x = \"\u{202E}hello\"";
    let error = Lexer::tokenize(source).expect_err("an unclosed override must fail");
    assert!(
        error.message.contains("U+202E") && error.message.contains("string literal"),
        "{}",
        error.message
    );
}

/// The same guard applies inside a `~` comment.
#[test]
fn test_unterminated_bidi_control_in_a_comment_errors() {
    let source = "x = 1 ~ \u{202B}gone wrong\ny = 2";
    let error = Lexer::tokenize(source).expect_err("an unclosed embedding must fail");
    assert!(
        error.message.contains("U+202B") && error.message.contains("comment"),
        "{}",
        error.message
    );
}

/// A bidi control appearing outside any string literal or comment is a lex error.
#[test]
fn test_bidi_control_outside_a_token_errors() {
    let source = "x \u{202E} = 1";
    let error = Lexer::tokenize(source).expect_err("a bare bidi control must fail");
    assert!(error.message.contains("U+202E"), "{}", error.message);
}

/// LRM (a scopeless mark) needs no closer and lexes fine inside a literal.
#[test]
fn test_lrm_inside_a_literal_lexes() {
    let source = "x = \"a\u{200E}b\"";
    assert!(Lexer::tokenize(source).is_ok());
}

/// A closer with nothing open to close (PDF or PDI) is a stray-closer lex error.
#[test]
fn test_closer_with_no_opener_errors() {
    for source in ["x = \"a\u{202C}b\"", "x = \"a\u{2069}b\""] {
        let error = Lexer::tokenize(source).expect_err("a stray closer must fail");
        assert!(
            error.message.contains("no matching opener"),
            "{}",
            error.message
        );
    }
}

/// A closer of the wrong family (PDF over an isolate, PDI over an embedding) is also a
/// stray closer — UAX #9 nesting requires each closer to match the innermost scope's own
/// kind, not just any open scope.
#[test]
fn test_mismatched_bidi_closer_family_errors() {
    let source = "\"\u{2067}x\u{202C}\u{2069}\""; // RLI ... PDF (wrong family) ... PDI
    let error = Lexer::tokenize(source).expect_err("a wrong-family closer must fail");
    assert!(
        error.message.contains("no matching opener"),
        "{}",
        error.message
    );
}

/// A legitimate Hebrew/Arabic literal — no bidi controls, just RTL letters — lexes exactly
/// like any other string.
#[test]
fn test_legitimate_rtl_literal_is_unaffected() {
    let text = "שלום مرحبا"; // Hebrew "hello" + Arabic "hello", no bidi controls
    let source = format!("\"{text}\"");
    let tokens = Lexer::tokenize(&source).expect("plain RTL text must lex");
    match &tokens[0].kind {
        TokenKind::String(chunks) => assert_eq!(
            chunks.as_slice(),
            [quilon::lexer::StrChunk::Lit(text.to_string())]
        ),
        other => panic!("expected a string token, got {other:?}"),
    }
}

#[test]
fn test_parsed_nodes_inherit_their_files_id() {
    // Composed spans (a BinaryOperator over two operands, a call over its arguments) are built
    // by the parser rather than copied from a token, so they have to inherit the id too.
    let tokens = Lexer::tokenize_in_file("f = (a :: Num) -> Num => < a + 1 * 2 >", 7).unwrap();
    let program = parse(&tokens).unwrap();
    let Item::FunctionDeclaration(declaration) = &program.items[0] else {
        panic!("expected a function declaration");
    };
    let Expression::Block { statements, .. } = &declaration.body else {
        panic!("expected a block body");
    };
    let Some(Statement::Expression(tail)) = statements.last() else {
        panic!("expected a tail expression");
    };
    let mut spans = vec![declaration.body.span().clone(), tail.span().clone()];
    if let Expression::BinaryOperator { left, right, .. } = tail {
        spans.push(left.span().clone());
        spans.push(right.span().clone());
    } else {
        panic!("expected the tail to be a binary operation");
    }
    assert!(
        spans.iter().all(|s| s.file == 7),
        "every node's span keeps its file: {:?}",
        spans
    );
}
