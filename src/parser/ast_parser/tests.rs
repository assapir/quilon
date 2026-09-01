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
    let tokens = Lexer::tokenize("<< \"lib/math.qn\"\n^ = () -> Num => 0").unwrap();
    let program = parse(&tokens).unwrap();
    assert_eq!(program.imports.len(), 1);
    match &program.imports[0].path {
        ModulePath::FilePath(p) => assert_eq!(p, "lib/math.qn"),
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
        Item::FunctionDeclaration(f) => {
            assert_eq!(f.name, "add");
            assert!(f.exported, "`>> add` should be exported");
        }
        other => panic!("expected FunctionDeclaration, got {:?}", other),
    }
    match &program.items[1] {
        Item::FunctionDeclaration(f) => assert!(!f.exported, "`helper` should be private"),
        other => panic!("expected FunctionDeclaration, got {:?}", other),
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
    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        assert!(declaration.mutable);
        assert_eq!(declaration.name, "counter");
    } else {
        panic!("Expected VariableDeclaration");
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
    // A `name :: Type = expression` binding INSIDE a `< >` block must parse and carry its
    // annotation, exactly like the top-level `x :: Num = 42` form above. (Regression:
    // the block parser used to only recognize `=`/`:=` bindings, choking on `::`.)
    use crate::ast::{Expression, Statement};
    let tokens = Lexer::tokenize("^ = () -> Num => <\n  n :: Num = 5\n  n\n>").unwrap();
    let program = parse(&tokens).expect("block-level annotated binding should parse");
    let Item::FunctionDeclaration(func) = &program.items[0] else {
        panic!("expected the `^` function declaration");
    };
    let Expression::Block { statements, .. } = &func.body else {
        panic!("expected a block body");
    };
    let Statement::Item(Item::VariableDeclaration(declaration)) = &statements[0] else {
        panic!("expected the first statement to be an annotated VariableDeclaration");
    };
    assert_eq!(declaration.name, "n");
    assert_eq!(declaration.type_annotation, Some(crate::ast::Type::Num));
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
    // `<` after a complete operand is `Lt`; a `>` with an operand after it is `Gt`.
    let lt = parse(&Lexer::tokenize("flag = a < b").unwrap()).unwrap();
    if let Item::VariableDeclaration(d) = &lt.items[0] {
        assert!(matches!(
            d.value,
            Expression::BinaryOperator {
                operator: BinaryOperator::Lt,
                ..
            }
        ));
    } else {
        panic!("expected a var declaration");
    }
    let gt = parse(&Lexer::tokenize("flag = a > b").unwrap()).unwrap();
    if let Item::VariableDeclaration(d) = &gt.items[0] {
        assert!(matches!(
            d.value,
            Expression::BinaryOperator {
                operator: BinaryOperator::Gt,
                ..
            }
        ));
    } else {
        panic!("expected a var declaration");
    }
}

#[test]
fn test_parse_operator_definition() {
    // An operator symbol can name a top-level definition (an operator overload).
    let tokens =
        Lexer::tokenize("P = { x :: Num }\n== = (a :: P, b :: P) -> Bool => a.x == b.x").unwrap();
    let program = parse(&tokens).unwrap();
    // Items: the type declaration and the `==` operator function.
    let operator = program.items.iter().find_map(|i| match i {
        Item::FunctionDeclaration(f) if f.name == "==" => Some(f),
        _ => None,
    });
    let operator = operator.expect("expected an `==` operator definition");
    assert_eq!(operator.parameters.len(), 2);
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
    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        if let Expression::Match { arms, .. } = &declaration.value {
            assert_eq!(arms.len(), 2);
        } else {
            panic!("Expected Match expression");
        }
    } else {
        panic!("Expected VariableDeclaration");
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
    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        if let Expression::Record { fields, .. } = &declaration.value {
            assert_eq!(fields.len(), 2);
        } else {
            panic!("Expected Record expression");
        }
    } else {
        panic!("Expected VariableDeclaration");
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
    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        if let Expression::Constructor {
            type_name, fields, ..
        } = &declaration.value
        {
            assert_eq!(type_name, "User");
            assert_eq!(fields.len(), 2);
        } else {
            panic!(
                "Expected Constructor expression, got {:?}",
                declaration.value
            );
        }
    } else {
        panic!("Expected VariableDeclaration");
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

    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        // The root should be BinaryOperator(Add)
        if let Expression::BinaryOperator {
            operator: BinaryOperator::Add,
            ..
        } = &declaration.value
        {
            // Correct precedence
        } else {
            panic!("Expected Add at root, got {:?}", declaration.value);
        }
    }
}

#[test]
fn test_parse_simple_function() {
    let tokens = Lexer::tokenize("add = (a, b) => a + b").unwrap();
    let result = parse(&tokens);
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::FunctionDeclaration(func) = &program.items[0] {
        assert_eq!(func.name, "add");
        assert_eq!(func.parameters.len(), 2);
        assert_eq!(func.parameters[0].name, "a");
        assert_eq!(func.parameters[1].name, "b");
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
    if let Item::FunctionDeclaration(func) = &program.items[0] {
        assert_eq!(func.parameters.len(), 2);
        assert!(func.parameters[0].type_annotation.is_some());
        assert!(func.return_type.is_some());
    } else {
        panic!("Expected function declaration");
    }
}

#[test]
fn test_parse_no_parameter_function() {
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
    if let Item::FunctionDeclaration(func) = &program.items[0] {
        if let Expression::Block { statements, .. } = &func.body {
            assert_eq!(statements.len(), 2);
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
fn test_parse_no_parameter_function_with_return_type() {
    let tokens = Lexer::tokenize("greet = () -> Text => \"Hello\"").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::FunctionDeclaration(func) = &program.items[0] {
        assert_eq!(func.parameters.len(), 0);
        assert!(func.return_type.is_some());
        if let Some(Type::Text) = func.return_type {
            // Success
        } else {
            panic!("Expected Text return type");
        }
    } else {
        panic!("Expected FunctionDeclaration");
    }
}

#[test]
fn test_parse_infix_range() {
    // `1 <- 4` in general expression position parses as an Expression::Range.
    let tokens = Lexer::tokenize("r = 1 <- 4").unwrap();
    let program = parse(&tokens).expect("range should parse");
    if let Item::VariableDeclaration(v) = &program.items[0] {
        assert!(
            matches!(v.value, Expression::Range { .. }),
            "expected Expression::Range, got {:?}",
            v.value
        );
    } else {
        panic!("expected a var declaration");
    }
}

#[test]
fn test_for_is_now_a_plain_identifier() {
    // The `for` loop was removed: `for` is no longer a keyword, so it lexes as
    // an ordinary identifier and a `for n <- ...` header no longer forms a loop.
    // Here `for` is just a bound name.
    let tokens = Lexer::tokenize("for = 42").unwrap();
    let program = parse(&tokens).expect("`for` should parse as a plain binding");
    if let Item::VariableDeclaration(v) = &program.items[0] {
        assert_eq!(v.name, "for");
        assert!(matches!(v.value, Expression::Number { .. }));
    } else {
        panic!("expected a var declaration binding the identifier `for`");
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
    if let Item::VariableDeclaration(var) = &program.items[0] {
        // Should be desugared to a Call with Ident("getName") as the function
        if let Expression::Call {
            function,
            arguments,
            ..
        } = &var.value
        {
            // function should be Ident("getName")
            if let Expression::Identifier { name, .. } = function.as_ref() {
                assert_eq!(name, "getName");
                // First arg should be the receiver (user)
                assert_eq!(arguments.len(), 1);
                if let Expression::Identifier { name, .. } = &arguments[0] {
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
    if let Item::VariableDeclaration(var) = &program.items[0]
        && let Expression::Call {
            function,
            arguments,
            ..
        } = &var.value
        && let Expression::Identifier { name, .. } = function.as_ref()
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
fn test_parse_type_declaration_with_fields() {
    let tokens = Lexer::tokenize("User = { name :: Text, age :: Num }").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::TypeDeclaration(declaration) = &program.items[0] {
        assert_eq!(declaration.name, "User");
        if let TypeDefinition::Record { fields, methods } = &declaration.type_definition {
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
fn test_parse_type_declaration_with_methods() {
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
    if let Item::TypeDeclaration(declaration) = &program.items[0] {
        assert_eq!(declaration.name, "User");
        if let TypeDefinition::Record { fields, methods } = &declaration.type_definition {
            assert_eq!(fields.len(), 1);
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].name, "getName");
            assert_eq!(methods[0].parameters.len(), 0); // "it" is implicit
        } else {
            panic!("Expected Record type definition");
        }
    } else {
        panic!("Expected type declaration");
    }
}

#[test]
fn test_parse_type_declaration_method_with_parameters() {
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
    if let Item::TypeDeclaration(declaration) = &program.items[0]
        && let TypeDefinition::Record { fields: _, methods } = &declaration.type_definition
    {
        assert_eq!(methods[0].name, "incrementAge");
        assert_eq!(methods[0].parameters.len(), 1);
        assert_eq!(methods[0].parameters[0].name, "amount");
    }
}

#[test]
fn test_parse_type_declaration_method_with_parameters_as_first_member() {
    // The disambiguating lookahead only looked at the FIRST member; a method whose
    // parameter list is parenthesized (`(p :: Text) -> Text => ...`) put `(` right after
    // `=`, which used to fall through to the record-literal reading.
    let tokens = Lexer::tokenize(
        "Greeter = {
  label = (p :: Text) -> Text => p
  name :: Text
}",
    )
    .unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::TypeDeclaration(declaration) = &program.items[0]
        && let TypeDefinition::Record { fields, methods } = &declaration.type_definition
    {
        assert_eq!(fields.len(), 1);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "label");
        assert_eq!(methods[0].parameters.len(), 1);
    } else {
        panic!("Expected a Record type declaration");
    }
}

#[test]
fn test_parse_type_declaration_render_member_as_first_member() {
    // The render operator `` ` `` as the first member failed even earlier — the old scan
    // only recognized an Ident there.
    let tokens = Lexer::tokenize("Point = { ` = () -> Text => \"pt\", x :: Num }").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::TypeDeclaration(declaration) = &program.items[0]
        && let TypeDefinition::Record { fields, methods } = &declaration.type_definition
    {
        assert_eq!(fields.len(), 1);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "`");
    } else {
        panic!("Expected a Record type declaration");
    }
}

#[test]
fn test_parse_record_literal_first_field_parenthesized_expression_stays_literal() {
    // `ident = (` is genuinely ambiguous with a method's parameter list; a parenthesized
    // VALUE expression as a record literal's first field must still parse as a literal.
    let tokens = Lexer::tokenize("point = { x = (1 + 2), y = 3 }").unwrap();
    let result = parse(&tokens);
    if let Err(e) = result.as_ref() {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());

    let program = result.unwrap();
    if let Item::VariableDeclaration(declaration) = &program.items[0] {
        if let Expression::Record { fields, .. } = &declaration.value {
            assert_eq!(fields.len(), 2);
        } else {
            panic!("Expected Record expression");
        }
    } else {
        panic!("Expected VariableDeclaration");
    }
}

/// The body statements of the single `^` function in `src` (which must be a block).
fn entry_block_statements(src: &str) -> Vec<Statement> {
    let tokens = Lexer::tokenize(src).unwrap();
    let program = parse(&tokens).expect("program should parse");
    let Item::FunctionDeclaration(func) = &program.items[0] else {
        panic!("expected the `^` function declaration");
    };
    let Expression::Block { statements, .. } = &func.body else {
        panic!("expected a block body");
    };
    statements.clone()
}

#[test]
fn test_line_first_paren_starts_new_statement() {
    // Statement-boundary rule: a `(` that opens a line never continues the previous
    // expression as a call. `x = f()` followed by a line `(1 + 2)` is TWO
    // statements, not the fused call `f()(1 + 2)`.
    let statements = entry_block_statements("^ = () -> Num => <\n  x = f()\n  (1 + 2)\n  x\n>");
    assert_eq!(statements.len(), 3);
    let Statement::Item(Item::VariableDeclaration(declaration)) = &statements[0] else {
        panic!(
            "expected `x = f()` as a VariableDeclaration, got {:?}",
            statements[0]
        );
    };
    let Expression::Call { arguments, .. } = &declaration.value else {
        panic!(
            "expected `x`'s value to be the call `f()`, got {:?}",
            declaration.value
        );
    };
    assert!(
        arguments.is_empty(),
        "the call must not swallow `(1 + 2)` as an argument"
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Expression(Expression::BinaryOperator { .. })
        ),
        "the line-first `(1 + 2)` must be its own statement, got {:?}",
        statements[1]
    );
}

#[test]
fn test_line_first_bracket_starts_new_statement() {
    // Same rule for `[`: `b = a` followed by a line `[3, 4].each(f)` is TWO
    // statements, not the fused index `a[3, 4]`.
    let statements =
        entry_block_statements("^ = () -> Num => <\n  b = a\n  [3, 4].each(f)\n  b\n>");
    assert_eq!(statements.len(), 3);
    let Statement::Item(Item::VariableDeclaration(declaration)) = &statements[0] else {
        panic!(
            "expected `b = a` as a VariableDeclaration, got {:?}",
            statements[0]
        );
    };
    assert!(
        matches!(&declaration.value, Expression::Identifier { .. }),
        "`b`'s value must stay the plain `a`, not become an index, got {:?}",
        declaration.value
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Expression(Expression::Call { .. })
        ),
        "the line-first `[3, 4].each(f)` must be its own statement, got {:?}",
        statements[1]
    );
}

#[test]
fn test_line_first_brace_starts_new_statement() {
    // Same rule for `{`: `b = a` followed by a line `{ x = 1 }` is TWO statements,
    // not the fused record constructor `a { x = 1 }`.
    let statements = entry_block_statements("^ = () -> Num => <\n  b = a\n  { x = 1 }\n  b\n>");
    assert_eq!(statements.len(), 3);
    let Statement::Item(Item::VariableDeclaration(declaration)) = &statements[0] else {
        panic!(
            "expected `b = a` as a VariableDeclaration, got {:?}",
            statements[0]
        );
    };
    assert!(
        matches!(&declaration.value, Expression::Identifier { .. }),
        "`b`'s value must stay the plain `a`, not become a constructor, got {:?}",
        declaration.value
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Expression(Expression::Record { .. })
        ),
        "the line-first `{{ x = 1 }}` must be its own record statement, got {:?}",
        statements[1]
    );
}

#[test]
fn test_same_line_constructor_still_builds() {
    // The rule only gates a LINE-FIRST `{`. A `{` on the type's own line is still a
    // record constructor, and its field body may span lines.
    let statements = entry_block_statements(
        "^ = () -> Num => <\n  p = Point {\n    x = 3,\n    y = 4\n  }\n  p\n>",
    );
    let Statement::Item(Item::VariableDeclaration(declaration)) = &statements[0] else {
        panic!(
            "expected `p = Point {{...}}` as a VariableDeclaration, got {:?}",
            statements[0]
        );
    };
    let Expression::Constructor {
        type_name, fields, ..
    } = &declaration.value
    else {
        panic!(
            "same-line `{{` must build a constructor, got {:?}",
            declaration.value
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
    let Item::VariableDeclaration(declaration) = &program.items[0] else {
        panic!("expected a VariableDeclaration");
    };
    let Expression::Call { arguments, .. } = &declaration.value else {
        panic!("expected a call, got {:?}", declaration.value);
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
    let Item::VariableDeclaration(declaration) = &program.items[0] else {
        panic!("expected a VariableDeclaration");
    };
    // `xs.map(f).filter(g)` desugars to `filter(map(xs, f), g)`.
    let Expression::Call {
        function,
        arguments,
        ..
    } = &declaration.value
    else {
        panic!("expected the chained call, got {:?}", declaration.value);
    };
    assert!(
        matches!(function.as_ref(), Expression::Identifier { name, .. } if name == "filter"),
        "outermost call should be `filter`"
    );
    assert_eq!(arguments.len(), 2);
}
