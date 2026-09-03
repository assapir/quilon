//! Written types: `Num`, `[]T`, named types, and the sum-type alternatives after `/`.
//!
//! Part of the recursive-descent parser; see `super` for the `Parser` cursor these
//! methods run against.

use super::*;

impl<'a> Parser<'a> {
    /// Depth-guarded entry point for type parsing. Type syntax recurses independently
    /// of the expression grammar (`[]T` element types), so it needs the same
    /// `MAX_NESTING_DEPTH` bound to keep `[][]…[]T` from overflowing the stack.
    pub(super) fn parse_type(&mut self) -> Result<crate::ast::Type, ParseError> {
        self.nested(Self::parse_type_inner)
    }

    pub(super) fn parse_type_inner(&mut self) -> Result<crate::ast::Type, ParseError> {
        // A qualified type — `http.Request` in an annotation — reads as one dotted name;
        // the checker resolves it like any other named reference. Tried first: the chain
        // only matches through an import binding, so no built-in type name is shadowed.
        if let Some((name, _span)) = self.try_parse_module_member() {
            return Ok(crate::ast::Type::named_ref(name));
        }

        let token = self.peek();

        // `$` in type position is the Unit type (e.g. `-> $`). Matched on the token
        // kind rather than its text since `$` is a dedicated token, not an identifier.
        if token.kind == TokenKind::Unit {
            self.advance();
            return Ok(crate::ast::Type::Unit);
        }

        // A pipe fence `[| … |]` (a `[` immediately followed by `|`) opens a Map or Set
        // type: `[|K => V|]` is `Map(K, V)`, `[|T|]` is `Set(T)`. Checked BEFORE the plain
        // `[]T` array type, which also begins with `[`.
        if token.kind == TokenKind::BracketOpen && self.peek_ahead(1).kind == TokenKind::Pipe {
            return self.parse_fenced_type();
        }

        // `( … ) ->` — a function type: `() -> $`, `(Num) -> Bool`, `(Num, Text) -> Bool`.
        // Parentheses appear in type position only here (the language has no grouped or
        // tuple types), so an opening paren always begins a function type. A function type
        // may itself be a parameter type (`((Num) -> Bool, Num) -> Bool`).
        if token.kind == TokenKind::ParenOpen {
            return self.parse_function_type();
        }

        // `[]T` — an array type (e.g. `[]Text`, and nested `[][]Text`). The `[]` prefix
        // wraps the element type that follows, so `[][]Text` parses as
        // `Array(Array(Text))` via the recursive `parse_type` call.
        if token.kind == TokenKind::BracketOpen {
            self.advance();
            self.expect(&TokenKind::BracketClose)?;
            let elem = self.parse_type()?;
            return Ok(crate::ast::Type::Array(Box::new(elem)));
        }

        match token.text.as_str() {
            "Num" => {
                self.advance();
                Ok(crate::ast::Type::Num)
            }
            "Text" => {
                self.advance();
                Ok(crate::ast::Type::Text)
            }
            "Bool" => {
                self.advance();
                Ok(crate::ast::Type::Bool)
            }
            "Result" => {
                self.advance();
                // Optional generic arguments, e.g. `Result{T, E}` — consumed and
                // ignored for now (the builtin Result is monomorphic in codegen).
                if self.check(&TokenKind::BraceOpen) {
                    let mut depth = 0usize;
                    loop {
                        if self.check(&TokenKind::BraceOpen) {
                            depth += 1;
                            self.advance();
                        } else if self.check(&TokenKind::BraceClose) {
                            self.advance();
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        } else if self.is_at_end() {
                            break;
                        } else {
                            self.advance();
                        }
                    }
                }
                // Must match `add_builtins` in the type checker exactly so that a
                // declared `-> Result` is equal to an inferred `Ok(..)`/`NotOk(..)`
                // body type under `check_type_compatibility`.
                Ok(crate::ast::Type::Sum {
                    name: "Result".to_string(),
                    variants: vec![
                        crate::ast::SumVariant {
                            name: "Ok".to_string(),
                            fields: vec![crate::ast::Type::Generic {
                                name: "T".to_string(),
                                arguments: vec![],
                            }],
                        },
                        crate::ast::SumVariant {
                            name: "NotOk".to_string(),
                            fields: vec![crate::ast::Type::Generic {
                                name: "E".to_string(),
                                arguments: vec![],
                            }],
                        },
                    ],
                })
            }
            // Any other Capitalized identifier is a reference to a user-defined type
            // (e.g. a sum type `Color`/`Shape`). The type checker resolves it by name
            // against the registered types. Emitted as a `Named` reference with no
            // fields; the checker replaces it with the concrete definition.
            other if is_capitalized(other) => {
                let name = other.to_string();
                self.advance();
                Ok(crate::ast::Type::named_ref(name))
            }
            _ => Err(ParseError::new(
                Code::UnexpectedToken,
                token.span.clone(),
                format!("expected a type, found {}", token.kind.describe()),
            )),
        }
    }

    /// Parse a function type, with the cursor at the opening `(`:
    /// `(P1, P2, …) -> R`. The parameter list may be empty (`() -> $`) and each parameter
    /// type is itself a full type, as is the return — so a parameter or the result may be
    /// a function type, and `(A) -> (B) -> C` reads right-associatively as
    /// `(A) -> ((B) -> C)`.
    fn parse_function_type(&mut self) -> Result<crate::ast::Type, ParseError> {
        self.expect(&TokenKind::ParenOpen)?;
        let mut parameters = Vec::new();
        if !self.check(&TokenKind::ParenClose) {
            loop {
                parameters.push(self.parse_type()?);
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }
        self.expect(&TokenKind::ParenClose)?;
        self.expect(&TokenKind::ReturnArrow)?;
        let return_type = self.parse_type()?;
        Ok(crate::ast::Type::Function {
            parameters,
            return_type: Box::new(return_type),
        })
    }

    /// Parse a pipe-fenced collection type, with the cursor at the opening `[` of a `[|`.
    /// `[|K => V|]` is `Map(K, V)`; `[|T|]` is `Set(T)`. The `=>` after the first type is
    /// what distinguishes a map from a set.
    fn parse_fenced_type(&mut self) -> Result<crate::ast::Type, ParseError> {
        self.expect(&TokenKind::BracketOpen)?;
        self.expect(&TokenKind::Pipe)?;
        let first = self.parse_type()?;
        if self.check(&TokenKind::Arrow) {
            self.advance(); // `=>`
            let value = self.parse_type()?;
            self.expect_fence_close()?;
            Ok(crate::ast::Type::Map(Box::new(first), Box::new(value)))
        } else {
            self.expect_fence_close()?;
            Ok(crate::ast::Type::Set(Box::new(first)))
        }
    }

    /// Consume a closing pipe fence `|]` (a `|` immediately followed by `]`).
    pub(super) fn expect_fence_close(&mut self) -> Result<(), ParseError> {
        self.expect(&TokenKind::Pipe)?;
        self.expect(&TokenKind::BracketClose)
    }
}
