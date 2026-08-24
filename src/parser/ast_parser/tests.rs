use super::*;
use crate::ast::{Statement, Type};
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
fn test_parse_builtin_import() {
    let tokens = Lexer::tokenize("<< core.io\n^ = () -> Num => 0").unwrap();
    let program = parse(&tokens).unwrap();
    assert_eq!(program.imports.len(), 1);
    match &program.imports[0].path {
        ModulePath::BuiltinDotted(parts) => assert_eq!(parts, &["core", "io"]),
        other => panic!("expected BuiltinDotted, got {:?}", other),
    }
    assert_eq!(program.items.len(), 1);
}

#[test]
fn test_parse_file_path_import() {
    let tokens = Lexer::tokenize("<< \"lib/math.ql\"\n^ = () -> Num => 0").unwrap();
    let program = parse(&tokens).unwrap();
    assert_eq!(program.imports.len(), 1);
    match &program.imports[0].path {
        ModulePath::FilePath(p) => assert_eq!(p, "lib/math.ql"),
        other => panic!("expected FilePath, got {:?}", other),
    }
}

#[test]
fn test_parse_export_marker() {
    let tokens = Lexer::tokenize(">> add = (a, b) => a + b\nhelper = x => x").unwrap();
    let program = parse(&tokens).unwrap();
    assert_eq!(program.items.len(), 2);
    // First item is exported, second is private.
    match &program.items[0] {
        Item::FunctionDecl(f) => {
            assert_eq!(f.name, "add");
            assert!(f.exported, "`>> add` should be exported");
        }
        other => panic!("expected FunctionDecl, got {:?}", other),
    }
    match &program.items[1] {
        Item::FunctionDecl(f) => assert!(!f.exported, "`helper` should be private"),
        other => panic!("expected FunctionDecl, got {:?}", other),
    }
}

#[test]
fn test_parse_boolean() {
    let tokens = Lexer::tokenize("flag = true").unwrap();
    let result = parse(&tokens);
    assert!(result.is_ok());
}

#[test]
fn test_parse_mutable() {
    let tokens = Lexer::tokenize("counter := 0").unwrap();
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
fn test_parse_block_level_annotated_binding() {
    // A `name :: Type = expr` binding INSIDE a `< >` block must parse and carry its
    // annotation, exactly like the top-level `x :: Num = 42` form above. (Regression:
    // the block parser used to only recognize `=`/`:=` bindings, choking on `::`.)
    use crate::ast::{Expr, Statement};
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  n :: Num = 5\n  n\n>").unwrap();
    let program = parse(&tokens).expect("block-level annotated binding should parse");
    let Item::FunctionDecl(func) = &program.items[0] else {
        panic!("expected the `^` function decl");
    };
    let Expr::Block { stmts, .. } = &func.body else {
        panic!("expected a block body");
    };
    let Statement::Item(Item::VarDecl(decl)) = &stmts[0] else {
        panic!("expected the first statement to be an annotated VarDecl");
    };
    assert_eq!(decl.name, "n");
    assert_eq!(decl.type_annotation, Some(crate::ast::Type::Num));
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
fn test_parse_bare_less_and_greater_than() {
    // `<` after a complete operand is `Lt`; a non-line-final `>` is `Gt`.
    let lt = parse(&Lexer::tokenize("flag = a < b").unwrap()).unwrap();
    if let Item::VarDecl(d) = &lt.items[0] {
        assert!(matches!(d.value, Expr::BinOp { op: BinOp::Lt, .. }));
    } else {
        panic!("expected a var decl");
    }
    let gt = parse(&Lexer::tokenize("flag = a > b").unwrap()).unwrap();
    if let Item::VarDecl(d) = &gt.items[0] {
        assert!(matches!(d.value, Expr::BinOp { op: BinOp::Gt, .. }));
    } else {
        panic!("expected a var decl");
    }
}

#[test]
fn test_parse_operator_definition() {
    // An operator symbol can name a top-level definition (an operator overload).
    let tokens =
        Lexer::tokenize("P = { x :: Num }\n== = (a :: P, b :: P) -> Bool => a.x == b.x").unwrap();
    let program = parse(&tokens).unwrap();
    // Items: the type decl and the `==` operator function.
    let op = program.items.iter().find_map(|i| match i {
        Item::FunctionDecl(f) if f.name == "==" => Some(f),
        _ => None,
    });
    let op = op.expect("expected an `==` operator definition");
    assert_eq!(op.params.len(), 2);
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
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
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
fn test_parse_constructor() {
    let tokens = Lexer::tokenize("user = User { name = \"Alice\", age = 30 }").unwrap();
    let result = parse(&tokens);
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::VarDecl(decl) = &program.items[0] {
        if let Expr::Constructor {
            type_name, fields, ..
        } = &decl.value
        {
            assert_eq!(type_name, "User");
            assert_eq!(fields.len(), 2);
        } else {
            panic!("Expected Constructor expression, got {:?}", decl.value);
        }
    } else {
        panic!("Expected VarDecl");
    }
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
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
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
fn test_parse_no_param_function_with_return_type() {
    let tokens = Lexer::tokenize("greet = () -> Text => \"Hello\"").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::FunctionDecl(func) = &program.items[0] {
        assert_eq!(func.params.len(), 0);
        assert!(func.return_type.is_some());
        if let Some(Type::Text) = func.return_type {
            // Success
        } else {
            panic!("Expected Text return type");
        }
    } else {
        panic!("Expected FunctionDecl");
    }
}

#[test]
fn test_parse_infix_range() {
    // `1 <- 4` in general expression position parses as an Expr::Range.
    let tokens = Lexer::tokenize("r = 1 <- 4").unwrap();
    let program = parse(&tokens).expect("range should parse");
    if let Item::VarDecl(v) = &program.items[0] {
        assert!(
            matches!(v.value, Expr::Range { .. }),
            "expected Expr::Range, got {:?}",
            v.value
        );
    } else {
        panic!("expected a var decl");
    }
}

#[test]
fn test_for_is_now_a_plain_identifier() {
    // The `for` loop was removed: `for` is no longer a keyword, so it lexes as
    // an ordinary identifier and a `for n <- ...` header no longer forms a loop.
    // Here `for` is just a bound name.
    let tokens = Lexer::tokenize("for = 42").unwrap();
    let program = parse(&tokens).expect("`for` should parse as a plain binding");
    if let Item::VarDecl(v) = &program.items[0] {
        assert_eq!(v.name, "for");
        assert!(matches!(v.value, Expr::Number { .. }));
    } else {
        panic!("expected a var decl binding the identifier `for`");
    }
}

#[test]
fn test_parse_method_call() {
    let tokens = Lexer::tokenize("result = user.getName()").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::VarDecl(var) = &program.items[0] {
        // Should be desugared to a Call with Ident("getName") as the function
        if let Expr::Call {
            function,
            arguments,
            ..
        } = &var.value
        {
            // function should be Ident("getName")
            if let Expr::Ident { name, .. } = function.as_ref() {
                assert_eq!(name, "getName");
                // First arg should be the receiver (user)
                assert_eq!(arguments.len(), 1);
                if let Expr::Ident { name, .. } = &arguments[0] {
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
    if let Item::VarDecl(var) = &program.items[0]
        && let Expr::Call {
            function,
            arguments,
            ..
        } = &var.value
        && let Expr::Ident { name, .. } = function.as_ref()
    {
        assert_eq!(name, "setAge");
        // Should have 2 args: receiver and the argument
        assert_eq!(arguments.len(), 2);
    }
}

#[test]
fn test_parse_chained_method_calls() {
    let tokens = Lexer::tokenize("result = user.getName().toUpper()").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());
}

#[test]
fn test_parse_type_decl_with_fields() {
    let tokens = Lexer::tokenize("User = { name :: Text, age :: Num }").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
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
    let tokens = Lexer::tokenize(
        "User = {
  name :: Text,
  getName = => it.name
}",
    )
    .unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
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
    let tokens = Lexer::tokenize(
        "User = { 
  age :: Num,
  incrementAge = amount => it.age + amount
}",
    )
    .unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::TypeDecl(decl) = &program.items[0]
        && let TypeDef::Record { fields: _, methods } = &decl.type_def
    {
        assert_eq!(methods[0].name, "incrementAge");
        assert_eq!(methods[0].params.len(), 1);
        assert_eq!(methods[0].params[0].name, "amount");
    }
}

/// The body statements of the single `^` function in `src` (which must be a block).
fn entry_block_stmts(src: &str) -> Vec<Statement> {
    let tokens = Lexer::tokenize(src).unwrap();
    let program = parse(&tokens).expect("program should parse");
    let Item::FunctionDecl(func) = &program.items[0] else {
        panic!("expected the `^` function decl");
    };
    let Expr::Block { stmts, .. } = &func.body else {
        panic!("expected a block body");
    };
    stmts.clone()
}

#[test]
fn test_line_first_paren_starts_new_statement() {
    // Statement-boundary rule: a `(` that opens a line never continues the previous
    // expression as a call. `x = f()` followed by a line `(1 + 2)` is TWO
    // statements, not the fused call `f()(1 + 2)`.
    let stmts = entry_block_stmts("^ = () -> Num => <\n  x = f()\n  (1 + 2)\n  x\n>");
    assert_eq!(stmts.len(), 3);
    let Statement::Item(Item::VarDecl(decl)) = &stmts[0] else {
        panic!("expected `x = f()` as a VarDecl, got {:?}", stmts[0]);
    };
    let Expr::Call { arguments, .. } = &decl.value else {
        panic!(
            "expected `x`'s value to be the call `f()`, got {:?}",
            decl.value
        );
    };
    assert!(
        arguments.is_empty(),
        "the call must not swallow `(1 + 2)` as an argument"
    );
    assert!(
        matches!(&stmts[1], Statement::Expr(Expr::BinOp { .. })),
        "the line-first `(1 + 2)` must be its own statement, got {:?}",
        stmts[1]
    );
}

#[test]
fn test_line_first_bracket_starts_new_statement() {
    // Same rule for `[`: `b = a` followed by a line `[3, 4].each(f)` is TWO
    // statements, not the fused index `a[3, 4]`.
    let stmts = entry_block_stmts("^ = () -> Num => <\n  b = a\n  [3, 4].each(f)\n  b\n>");
    assert_eq!(stmts.len(), 3);
    let Statement::Item(Item::VarDecl(decl)) = &stmts[0] else {
        panic!("expected `b = a` as a VarDecl, got {:?}", stmts[0]);
    };
    assert!(
        matches!(&decl.value, Expr::Ident { .. }),
        "`b`'s value must stay the plain `a`, not become an index, got {:?}",
        decl.value
    );
    assert!(
        matches!(&stmts[1], Statement::Expr(Expr::Call { .. })),
        "the line-first `[3, 4].each(f)` must be its own statement, got {:?}",
        stmts[1]
    );
}

#[test]
fn test_line_first_brace_starts_new_statement() {
    // Same rule for `{`: `b = a` followed by a line `{ x = 1 }` is TWO statements,
    // not the fused record constructor `a { x = 1 }`.
    let stmts = entry_block_stmts("^ = () -> Num => <\n  b = a\n  { x = 1 }\n  b\n>");
    assert_eq!(stmts.len(), 3);
    let Statement::Item(Item::VarDecl(decl)) = &stmts[0] else {
        panic!("expected `b = a` as a VarDecl, got {:?}", stmts[0]);
    };
    assert!(
        matches!(&decl.value, Expr::Ident { .. }),
        "`b`'s value must stay the plain `a`, not become a constructor, got {:?}",
        decl.value
    );
    assert!(
        matches!(&stmts[1], Statement::Expr(Expr::Record { .. })),
        "the line-first `{{ x = 1 }}` must be its own record statement, got {:?}",
        stmts[1]
    );
}

#[test]
fn test_same_line_constructor_still_builds() {
    // The rule only gates a LINE-FIRST `{`. A `{` on the type's own line is still a
    // record constructor, and its field body may span lines.
    let stmts =
        entry_block_stmts("^ = () -> Num => <\n  p = Point {\n    x = 3,\n    y = 4\n  }\n  p\n>");
    let Statement::Item(Item::VarDecl(decl)) = &stmts[0] else {
        panic!(
            "expected `p = Point {{...}}` as a VarDecl, got {:?}",
            stmts[0]
        );
    };
    let Expr::Constructor {
        type_name, fields, ..
    } = &decl.value
    else {
        panic!(
            "same-line `{{` must build a constructor, got {:?}",
            decl.value
        );
    };
    assert_eq!(type_name, "Point");
    assert_eq!(fields.len(), 2, "both fields, across lines, belong to it");
}

#[test]
fn test_multiline_call_arguments_still_one_call() {
    // The rule only gates a LINE-FIRST `(`. An argument list opened on the same
    // line as the callee may still span lines: `add(40,` newline `2)`.
    let tokens = Lexer::tokenize("x = add(40,\n  2)").unwrap();
    let program = parse(&tokens).expect("multi-line argument list should parse");
    assert_eq!(program.items.len(), 1);
    let Item::VarDecl(decl) = &program.items[0] else {
        panic!("expected a VarDecl");
    };
    let Expr::Call { arguments, .. } = &decl.value else {
        panic!("expected a call, got {:?}", decl.value);
    };
    assert_eq!(arguments.len(), 2);
}

#[test]
fn test_method_chain_across_lines_still_continues() {
    // A continuation line that starts with `.` still chains: only line-first
    // `(` / `[` end the expression.
    let tokens = Lexer::tokenize("x = xs.map(f)\n  .filter(g)").unwrap();
    let program = parse(&tokens).expect("a `.`-led continuation line should parse");
    assert_eq!(program.items.len(), 1);
    let Item::VarDecl(decl) = &program.items[0] else {
        panic!("expected a VarDecl");
    };
    // `xs.map(f).filter(g)` desugars to `filter(map(xs, f), g)`.
    let Expr::Call {
        function,
        arguments,
        ..
    } = &decl.value
    else {
        panic!("expected the chained call, got {:?}", decl.value);
    };
    assert!(
        matches!(function.as_ref(), Expr::Ident { name, .. } if name == "filter"),
        "outermost call should be `filter`"
    );
    assert_eq!(arguments.len(), 2);
}
