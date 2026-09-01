//! Declarations and the program itself: imports, top-level items, functions, named
//! record and sum types, and `< >` blocks.
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

/// A parsed `{ … }` member block: the fields (`name :: Type`) and methods, in order.
/// A record uses both; a sum's block is methods only (a field there is rejected).
type MemberBlock = (Vec<(String, crate::ast::Type)>, Vec<MethodDeclaration>);

impl<'a> Parser<'a> {
    pub fn parse(tokens: &'a [Token]) -> Result<Program, ParseError> {
        let mut parser = Self::new(tokens);
        parser.parse_program()
    }

    pub(super) fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut imports = Vec::new();
        let mut items = Vec::new();
        let mut test_blocks = Vec::new();

        while !self.is_at_end() {
            if self.check(&TokenKind::Import) {
                imports.push(self.parse_import()?);
            } else if self.at_test_block() {
                test_blocks.push(self.parse_expression()?);
            } else {
                items.push(self.parse_item()?);
            }
        }

        Ok(Program {
            imports,
            items,
            test_blocks,
        })
    }

    /// Whether the cursor is on a top-level test block: a CALL of the harness's
    /// [`crate::ast::TEST_BLOCK_MARKER`] — `test.describe("…", () => < … >)`, or the full
    /// `core.test.describe(...)` — with `core.test` imported above. A qualified name
    /// followed by anything but an argument list is an ordinary reference, and a BARE
    /// `describe(` is an ordinary call of whatever `describe` is in scope: only the
    /// harness's own, reached through its module, marks test code.
    fn at_test_block(&self) -> bool {
        if !self.at_possible_module_chain() {
            return false;
        }
        let segments = self.dotted_chain_at_cursor();
        let Some((canonical, member, prefix_len)) = self.resolve_chain(&segments) else {
            return false;
        };
        format!("{canonical}.{member}") == crate::ast::TEST_BLOCK_MARKER
            && segments.len() == prefix_len + 1
            && self.check_same_line_at(2 * prefix_len + 1, &TokenKind::ParenOpen)
    }

    /// Parse an import line: `<< core.io` (built-in dotted name) or `<< "path/to/mod.qn"` (file
    /// path).
    pub(super) fn parse_import(&mut self) -> Result<Import, ParseError> {
        let start = self.current_span();
        self.expect(&TokenKind::Import)?;

        let path = if let TokenKind::String(chunks) = self.peek().kind.clone() {
            // File-path import: << "some/path.qn". A path is a plain literal — an
            // interpolation hole here is meaningless, so reject it clearly.
            let span = self.peek().span.clone();
            self.advance();
            match chunks.as_slice() {
                [StrChunk::Lit(s)] => ModulePath::FilePath(s.clone()),
                _ => {
                    return Err(ParseError {
                        message: "import path cannot contain interpolation".to_string(),
                        span,
                    });
                }
            }
        } else {
            // Built-in dotted import: << core.io
            let mut parts = vec![self.expect_ident()?];
            while self.check(&TokenKind::Dot) {
                self.advance();
                parts.push(self.expect_ident()?);
            }
            ModulePath::BuiltinDotted(parts)
        };

        self.register_import(&path);
        let end = self.previous_span();
        Ok(Import {
            path,
            span: self.span(start.start, end.end),
        })
    }

    pub(super) fn parse_item(&mut self) -> Result<Item, ParseError> {
        // Three possibilities:
        // 1. Type declaration: Name = { fields and methods }
        // 2. Function declaration: name = parameters => body
        // 3. Variable declaration: name = value

        let start = self.current_span();

        // Optional `>>` export prefix: marks this top-level item as exported from its module.
        let exported = if self.check(&TokenKind::Export) {
            self.advance();
            true
        } else {
            false
        };

        // A top-level definition may be named by an operator symbol — this is how a
        // user declares an operator overload, e.g. `+ = (a :: Point, b :: Point) ...`.
        // An operator name is always a function definition (operators take operands).
        if let Some(op_name) = self.operator_def_name() {
            self.advance();
            self.expect(&TokenKind::Assign)?;
            return self.parse_function_declaration(op_name, start, None, exported);
        }

        // A leaf IO primitive declaration names itself with a fused `@name` (`@sleep`),
        // mirroring the call-site surface. Only the corelib declares these; user code is
        // rejected downstream (the front end refuses an `@` declaration outside a built-in
        // module). A `@name` is always a function declaration (a primitive takes args).
        let name = if self.check(&TokenKind::At) {
            self.advance();
            format!("@{}", self.expect_ident()?)
        } else {
            self.expect_ident()?
        };

        // The top level takes only declarations, so an `Ident.` here can only be a
        // qualified reference through an import this file does not have (had it, the
        // chain would have been consumed as one name before ever reaching parse_item).
        // The common case is a test suite missing its harness: name the fix.
        if self.check_same_line(&TokenKind::Dot) {
            return Err(ParseError {
                message: format!(
                    "`{name}` is not an imported module here — a qualified name like \
                     `{name}.<member>(...)` needs its `<<` import above this line \
                     (a `test.describe` suite imports `<< core.test`)"
                ),
                span: self.current_span(),
            });
        }

        // Check for type annotation
        let type_annotation = if self.check(&TokenKind::TypeAnnotation) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        // Binding operator: `=` (immutable bind) or `:=` (mutable bind / reassign).
        let mutable = if self.check(&TokenKind::MutAssign) {
            self.advance();
            true
        } else {
            self.expect(&TokenKind::Assign)?;
            false
        };

        // A `:=` binding is always a mutable value binding (or a reassignment of one);
        // it is never a type or function declaration.
        if mutable {
            let value = self.parse_expression()?;
            let end = self.previous_span();
            return Ok(Item::VariableDeclaration(VariableDeclaration {
                mutable: true,
                name,
                type_annotation,
                value,
                exported,
                span: self.span(start.start, end.end),
            }));
        }

        // Check if it's a type declaration (Name = { ... })
        // Type declarations can't be mutable and don't have type annotations
        // AND they must have field declarations (name :: Type) or methods (name = => ...)
        if type_annotation.is_none() && self.check(&TokenKind::BraceOpen) {
            // What opens the brace says which one this is: named by an ordinary identifier
            // or the render operator `` ` ``, a first member can be a field (`{ name :: Type
            // … }`) — unambiguous, always a type declaration — or a method, parameterless
            // (`{ name = => … }`) or with parameters (`{ name = (p :: T) -> R => … }`).
            // `name = (` alone is genuinely ambiguous with a parenthesized literal expression
            // (`x = (1 + 2)`), so that shape needs the same real scan to the matching `)`
            // that a lambda's parameter list uses.
            let first_member_is_field = matches!(
                self.peek_ahead(1).kind,
                TokenKind::Ident | TokenKind::Backtick
            ) && self.peek_ahead(2).kind == TokenKind::TypeAnnotation;
            let first_member_is_method = matches!(
                self.peek_ahead(1).kind,
                TokenKind::Ident | TokenKind::Backtick
            ) && self.peek_ahead(2).kind == TokenKind::Assign
                && match self.peek_ahead(3).kind {
                    TokenKind::Arrow => true,
                    TokenKind::ParenOpen => self.parameter_list_ahead_from(3),
                    _ => false,
                };

            if first_member_is_field {
                return self.parse_type_declaration(name, start, exported);
            }

            // A method-shaped first member is ambiguous on its own (LOCKED: content, never
            // the name's capitalization, decides — `x = { f = => 1 }` and `X = { f = => 1 }`
            // read the same way). A `::` field anywhere else in the block settles it as a
            // type declaration (a method may legitimately come before the fields it uses,
            // see `examples/methods.qn`); with no field at all, a block of nothing
            // but methods is neither reading unambiguously and is a compile error rather
            // than a silent guess.
            if first_member_is_method {
                if self.member_block_has_field() {
                    return self.parse_type_declaration(name, start, exported);
                }
                return Err(ParseError {
                    message: format!(
                        "`{name} = {{ … }}` is ambiguous — every member here is method-shaped \
                         and there is no `::` field to settle it. Add a `::` field to make \
                         this a type declaration (e.g. `{name} = {{ v :: Num, f = => 1 }}`), \
                         or replace the method bodies with plain values to make it a record \
                         literal (e.g. `{name} = {{ f = 1 }}`)"
                    ),
                    span: self.current_span(),
                });
            }
        }

        // Check if it's a sum-type declaration: `Name = VariantA / VariantB(Num) / ...`.
        // Disambiguation (LOCKED): `/` is a sum-type separator (not division) only when
        // the declared name and every operand are Capitalized type/constructor names. We
        // require the type name to be Capitalized and the RHS to be a `/`-separated list
        // of Capitalized constructors (each optionally taking a parenthesized payload list).
        // A single bare `Red` (no `/`) is a normal value binding, not a one-variant sum.
        if type_annotation.is_none() && is_capitalized(&name) && self.looks_like_sum_declaration() {
            return self.parse_sum_type_declaration(name, start, exported);
        }

        // Check if it's a function:
        // - name = => ...  (no parameters)
        // - name = (parameters) => ...
        // - name = parameter => ...  (single parameter, no parens)
        // Need to be careful not to confuse with: result = (2 + 3) * 4

        let is_function = if self.check(&TokenKind::Arrow) {
            true
        } else if self.check(&TokenKind::ParenOpen) {
            // A parameter list (ending `) =>` or `) ->`) rather than a parenthesized
            // expression — the same question a lambda in expression position asks.
            self.parameter_list_ahead()
        } else if self.check(&TokenKind::Ident) {
            // Single parameter without parens: followed by `=>` (body), `::` (parameter type),
            // or `->` (return type, e.g. `print = x -> $ => $`).
            let ahead = self.peek_ahead(1);
            ahead.kind == TokenKind::Arrow
                || ahead.kind == TokenKind::TypeAnnotation
                || ahead.kind == TokenKind::ReturnArrow
        } else {
            false
        };

        if is_function {
            self.parse_function_declaration(name, start, type_annotation, exported)
        } else {
            let value = self.parse_expression()?;
            let end = self.previous_span();

            Ok(Item::VariableDeclaration(VariableDeclaration {
                mutable,
                name,
                type_annotation,
                value,
                exported,
                span: self.span(start.start, end.end),
            }))
        }
    }

    /// Parse a function definition. `binding_type` is the `::` annotation written on the
    /// binding, if any — a function type there states the whole signature, anything else
    /// states the return type.
    pub(super) fn parse_function_declaration(
        &mut self,
        name: String,
        start: Span,
        binding_type: Option<crate::ast::Type>,
        exported: bool,
    ) -> Result<Item, ParseError> {
        // Parse parameters: (a, b) or (a :: Type, b :: Type) or single parameter or just =>
        let parameters = self.parse_parameter_list()?;

        // Optional return type annotation with ->. The `::` annotation on the binding is
        // kept as written beside it: what it states — the whole signature, or just the
        // return type — is read off its shape where types are understood, not here.
        let return_type = if self.check(&TokenKind::ReturnArrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        // Expect =>
        self.expect(&TokenKind::Arrow)?;

        // Parse body (can be a block or single expression)
        let body = if self.check(&TokenKind::BlockOpen) {
            self.parse_block()?
        } else {
            self.parse_expression()?
        };

        let end = self.previous_span();

        Ok(Item::FunctionDeclaration(FunctionDeclaration {
            name,
            parameters,
            return_type,
            binding_type,
            body,
            exported,
            // Parsing is provenance-blind: the module loader marks what it merges from a
            // built-in module, and the front end marks a corelib file checked directly.
            from_corelib: false,
            span: self.span(start.start, end.end),
        }))
    }

    pub(super) fn parse_type_declaration(
        &mut self,
        name: String,
        start: Span,
        exported: bool,
    ) -> Result<Item, ParseError> {
        // Parse type definition: Name = { field :: Type, ... method = => body, ... }
        let (fields, methods) = self.parse_member_block()?;
        let end = self.previous_span();

        Ok(Item::TypeDeclaration(TypeDeclaration {
            name,
            type_definition: TypeDefinition::Record { fields, methods },
            exported,
            span: self.span(start.start, end.end),
        }))
    }

    /// Parse a `{ … }` member block — the shared grammar of a record body and a sum type's
    /// optional trailing method block. Each member is either a field `name :: Type` or a
    /// method `name = parameters => body`. A member may be named by an operator symbol
    /// (`==`, `+`, …, the binary operators) or by the render operator `` ` ``, both of
    /// which are always methods; every other member name is an ordinary identifier.
    /// Returns the fields and methods in declaration order.
    pub(super) fn parse_member_block(&mut self) -> Result<MemberBlock, ParseError> {
        self.expect(&TokenKind::BraceOpen)?;

        let mut fields = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::BraceClose) && !self.is_at_end() {
            let member_name = if self.check(&TokenKind::Backtick) {
                self.advance();
                "`".to_string()
            } else if let Some(operator) = self.operator_def_name() {
                // An operator-symbol member (`== = …`, `+ = …`). `operator_def_name`
                // requires the following `=`, so this never swallows a stray operator.
                self.advance();
                operator
            } else if let Some(operator) = self.operator_symbol_name()
                && self.peek_ahead(1).kind == TokenKind::MutAssign
            {
                // Caught here so the author gets the rule rather than "expected identifier":
                // `operator_def_name` only recognizes `op = …`, so `op := …` would otherwise
                // fall through to the name parser and fail as a stray symbol.
                return Err(ParseError {
                    message: format!(
                        "operator member `{}` cannot be declared with `:=` — an operator yields a value and never mutates `it`",
                        operator
                    ),
                    span: self.peek().span.clone(),
                });
            } else {
                self.expect_ident()?
            };

            if self.check(&TokenKind::TypeAnnotation) {
                // This is a field: name :: Type
                self.advance();
                let field_type = self.parse_type()?;
                fields.push((member_name, field_type));
            } else if self.check(&TokenKind::Assign) || self.check(&TokenKind::MutAssign) {
                // A method: `name = params => body`, or `name := params => body` for one
                // that may mutate `it`.
                let mutating = self.check(&TokenKind::MutAssign);
                // An operator member yields a value; there is no receiver-mutability check
                // at an operator's use site, so declaring one mutating would be a promise
                // nothing enforces. (`operator_def_name` already refuses `+ := …` for the
                // symbol operators; the render member reaches here by its own branch.)
                if mutating && member_name == "`" {
                    return Err(ParseError {
                        message: "The render member ` cannot be declared with ':=' — it renders a value rather than mutating `it`".to_string(),
                        span: self.peek().span.clone(),
                    });
                }
                self.advance();

                let method_start = self.current_span();
                // Identical grammar to a function's parameters, so it is the same rule
                // ("it" is implicit and never listed here).
                let parameters = self.parse_parameter_list()?;

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
                    self.parse_expression()?
                };

                let method_end = self.previous_span();

                methods.push(MethodDeclaration {
                    name: member_name,
                    parameters,
                    return_type,
                    body,
                    mutating,
                    span: self.span(method_start.start, method_end.end),
                });
            } else {
                return Err(ParseError {
                    message: "Expected ::, = or := after field/method name".to_string(),
                    span: self.peek().span.clone(),
                });
            }

            // Optional comma separator
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
        }

        self.expect(&TokenKind::BraceClose)?;
        Ok((fields, methods))
    }

    /// Parse a sum-type declaration: `Name = VariantA / VariantB(Num, Text) / ...`.
    /// Each variant is a Capitalized constructor name with an optional parenthesized
    /// list of payload types (built-in types only — enforced by the type checker).
    pub(super) fn parse_sum_type_declaration(
        &mut self,
        name: String,
        start: Span,
        exported: bool,
    ) -> Result<Item, ParseError> {
        use crate::ast::{SumVariant, TypeDefinition};

        let mut variants = Vec::new();
        loop {
            let variant_name = self.expect_ident()?;
            if !is_capitalized(&variant_name) {
                return Err(ParseError {
                    message: format!(
                        "Sum-type variant '{}' must start with an uppercase letter",
                        variant_name
                    ),
                    span: self.previous_span(),
                });
            }

            // Optional payload-type list: `(Num)` or `(Num, Text)`.
            let mut fields = Vec::new();
            if self.check(&TokenKind::ParenOpen) {
                self.advance();
                if !self.check(&TokenKind::ParenClose) {
                    loop {
                        fields.push(self.parse_type()?);
                        if !self.check(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }
                self.expect(&TokenKind::ParenClose)?;
            }

            variants.push(SumVariant {
                name: variant_name,
                fields,
            });

            // Variants are separated by `/`; stop when the next token isn't one.
            if self.check(&TokenKind::Slash) {
                self.advance();
            } else {
                break;
            }
        }

        // Optional trailing `{ … }` method block. A sum has no fields, so a field-like
        // entry (`x :: Num`) there is rejected with a clear message; only methods are
        // allowed (named methods, the render `` ` ``, and operator members).
        let methods = if self.check(&TokenKind::BraceOpen) {
            let (fields, methods) = self.parse_member_block()?;
            if !fields.is_empty() {
                return Err(ParseError {
                    message: format!(
                        "sum type `{}` cannot have fields — its `{{ }}` block holds methods only (sums carry data in their variant payloads, not fields)",
                        name
                    ),
                    span: self.previous_span(),
                });
            }
            // A sum's receiver has no writable field — its data lives in variant payloads,
            // reached by matching, and a match binding is immutable. So `:=` here would
            // declare a mutation nothing can perform and nothing checks; the same reason
            // operator members refuse it. If payload mutation ever lands, allowing `:=`
            // then only widens what is accepted.
            if let Some(mutating) = methods.iter().find(|m| m.mutating) {
                return Err(ParseError {
                    message: format!(
                        "sum type `{}` cannot have a mutating method — `{}` is declared with `:=`, but a sum has no fields to write (its data lives in variant payloads)",
                        name, mutating.name
                    ),
                    span: mutating.span.clone(),
                });
            }
            methods
        } else {
            Vec::new()
        };

        let end = self.previous_span();
        Ok(Item::TypeDeclaration(TypeDeclaration {
            name,
            type_definition: TypeDefinition::Sum { variants, methods },
            exported,
            span: self.span(start.start, end.end),
        }))
    }

    /// Depth-guarded entry point for `< … >` blocks. A block may hold nested named
    /// function declarations whose bodies are themselves blocks
    /// (`f = () => < g = () => < … > >`); that recursion runs through
    /// `parse_item`/`parse_function_declaration` rather than `parse_expression`, so blocks get
    /// the `MAX_NESTING_DEPTH` bound here too.
    pub(super) fn parse_block(&mut self) -> Result<Expression, ParseError> {
        self.nested(Self::parse_block_inner)
    }

    pub(super) fn parse_block_inner(&mut self) -> Result<Expression, ParseError> {
        use crate::ast::Statement;

        let start = self.current_span();
        self.expect(&TokenKind::BlockOpen)?;

        let mut statements = Vec::new();

        while !self.check(&TokenKind::BlockClose) && !self.is_at_end() {
            // Two block closers written with no space between them (`… >>`) are one
            // `Export` token by maximal munch, and an export is never a statement, so say
            // what the fix is instead of failing further along on a phantom marker.
            if self.check(&TokenKind::Export) {
                return Err(ParseError {
                    message: "`>>` here is the export marker, not two block closers — \
                              separate them with a space (`> >`)"
                        .to_string(),
                    span: self.current_span(),
                });
            }

            // Try to parse as item first (for nested declarations / reassignments).
            // `name = …` is an immutable binding; `name := …` is a mutable bind/reassign;
            // `name :: Type = …` is an annotated binding. `::` at statement start is
            // unambiguously a binding annotation (there is no expression-level `::`), so
            // delegating to `parse_item` keeps block-level bindings identical to top-level.
            if self.check(&TokenKind::Ident)
                && matches!(
                    self.peek_ahead(1).kind,
                    TokenKind::Assign | TokenKind::MutAssign | TokenKind::TypeAnnotation
                )
            {
                // This looks like a declaration. A nested `name = parameters => body` stays an
                // `Item::FunctionDeclaration`; codegen decides per-declaration whether it is a capturing
                // CLOSURE or a plain (recursion-capable) local function, based on whether
                // it actually references enclosing locals.
                let item = self.parse_item()?;
                statements.push(Statement::Item(item));
            } else {
                statements.push(Statement::Expression(self.parse_expression()?));
            }

            // Expressions in blocks can be separated by newlines (already skipped by lexer)
            // or we just continue to the next one
        }

        self.expect(&TokenKind::BlockClose)?;
        let span = self.span(start.start, self.previous_span().end);

        Ok(Expression::Block { statements, span })
    }
}
