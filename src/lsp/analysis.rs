//! The language server's per-capability analysis, as pure functions over one front-end
//! run — no protocol types, no I/O beyond what the front end itself does — so each
//! capability is testable by calling its function directly.
//!
//! Positions here are byte offsets into the document's text; `crate::source_map`'s
//! [`DocumentPositions`](crate::source_map::DocumentPositions) translates them to and from
//! the protocol's UTF-16 line/column pairs at the server boundary.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::nodes::{
    Expression, InterpolationPart, Item, Parameter, Pattern, Program, RECEIVER, Statement,
    display_name, type_label,
};
use crate::driver::{self, Checked, FrontEndError, TestBlocks};
use crate::lexer::{Lexer, ROOT_FILE, Span, Token, TokenKind};
use crate::parser;
use crate::typechecker::TypeTable;

/// Run the compiler front end over `text` as the content of the document at `path`.
///
/// A test suite (top-level `describe` blocks and no `^` of its own) runs with its blocks
/// compiled, so diagnostics, hover, and definition reach inside the test bodies — matching
/// what `quilon test` executes. Any other document erases its blocks, matching every other
/// command.
pub fn check_text(path: &Path, text: &str) -> Result<Checked, FrontEndError> {
    let tests = match parses_as_test_suite(text) {
        true => TestBlocks::Run,
        false => TestBlocks::Erase,
    };
    driver::front_end_source(path, text.to_string(), tests)
}

/// Whether `text` parses as a test suite: top-level test blocks and no `^` entry point.
fn parses_as_test_suite(text: &str) -> bool {
    let Some(program) = parse_text(text) else {
        return false;
    };
    !program.test_blocks.is_empty() && !driver::has_entry_point(&program)
}

/// The document's own parse (pre-link, names as written), or `None` when it does not lex
/// or parse. Semantic tokens and test lenses read this: both speak about the text as
/// written, before the link pass renames anything.
pub fn parse_text(text: &str) -> Option<Program> {
    let tokens = Lexer::tokenize(text).ok()?;
    parser::parse(&tokens).ok()
}

// --- Go-to-definition, find references, rename ------------------------------

/// A name binding: a parameter, a block-local item, a pattern binding, or a top-level
/// item. `is_local` tells the two apart — a local's scope is lexical (only the reference
/// resolving to the very same declaration counts as "the same binding"), a top-level
/// name's is the whole program (every same-named item, the whole overload set, and every
/// use of that name count).
#[derive(Debug, Clone)]
pub struct Declaration {
    pub name: String,
    pub span: Span,
    pub is_local: bool,
}

/// One resolved identifier: the span of the `Expression::Identifier` as written, and the
/// declaration it resolves to.
#[derive(Debug, Clone)]
pub struct Reference {
    pub use_span: Span,
    pub declaration: Declaration,
}

/// Every name binding and resolved reference in `program`, in one walk. Go-to-definition,
/// find-references, and rename are all read off this same table — the compiler's own
/// notion of scope, restated once rather than re-derived per capability.
pub struct Resolver {
    pub references: Vec<Reference>,
    /// Every declaration the walk binds, root-file ones only — the document a client can
    /// rename or list references in. A top-level name contributes one entry per member of
    /// its overload set.
    pub declarations: Vec<Declaration>,
    /// Every top-level name (the document's and its imports'), by declaration — for an
    /// overload set, the first member stands for the set, matching a plain lookup's answer
    /// (there being no static way to tell which member an unresolved reference means).
    top_level: HashMap<String, Declaration>,
    /// The lexical scopes currently open, innermost last.
    scopes: Vec<HashMap<String, Declaration>>,
}

impl Resolver {
    /// Walk the whole (import-linked) `program` once, recording every reference and
    /// declaration it contains.
    pub fn walk(program: &Program) -> Self {
        let mut resolver = Resolver {
            references: Vec::new(),
            declarations: Vec::new(),
            top_level: HashMap::new(),
            scopes: Vec::new(),
        };
        for item in &program.items {
            let declaration = Declaration {
                name: item.name().to_string(),
                span: item.span().clone(),
                is_local: false,
            };
            resolver.record(declaration.clone());
            resolver
                .top_level
                .entry(declaration.name.clone())
                .or_insert(declaration);
        }
        for item in &program.items {
            resolver.item(item);
        }
        for block in &program.test_blocks {
            resolver.expression(block);
        }
        resolver
    }

    /// Record `declaration` into the flat table, when it names a position in this
    /// document — a declaration in an imported file plays no part in this document's
    /// references or rename.
    fn record(&mut self, declaration: Declaration) {
        if declaration.span.file == ROOT_FILE {
            self.declarations.push(declaration);
        }
    }

    fn lookup(&self, name: &str) -> Option<Declaration> {
        for scope in self.scopes.iter().rev() {
            if let Some(declaration) = scope.get(name) {
                return Some(declaration.clone());
            }
        }
        self.top_level.get(name).cloned()
    }

    fn bind(&mut self, name: &str, span: &Span) {
        let declaration = Declaration {
            name: name.to_string(),
            span: span.clone(),
            is_local: true,
        };
        self.record(declaration.clone());
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), declaration);
        }
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::VariableDeclaration(declaration) => self.expression(&declaration.value),
            Item::FunctionDeclaration(declaration) => {
                self.in_function(&declaration.parameters, &declaration.body)
            }
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    self.scopes.push(HashMap::new());
                    // The implicit receiver: its "declaration" is the method itself.
                    self.bind(RECEIVER, &method.span);
                    self.in_function(&method.parameters, &method.body);
                    self.scopes.pop();
                }
            }
        }
    }

    /// Walk a body with `parameters` in scope, each binding its own name at its own span.
    fn in_function(&mut self, parameters: &[Parameter], body: &Expression) {
        self.scopes.push(HashMap::new());
        for parameter in parameters {
            self.bind(&parameter.name, &parameter.span);
        }
        self.expression(body);
        self.scopes.pop();
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Item(item) => {
                // The declaration is in scope for its own body (self-recursion), and for
                // everything after it in the block.
                self.bind(item.name(), item.span());
                self.item(item)
            }
            Statement::Expression(expression) => self.expression(expression),
        }
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier { name, span } => {
                if let Some(declaration) = self.lookup(name) {
                    self.references.push(Reference {
                        use_span: span.clone(),
                        declaration,
                    });
                }
            }
            Expression::Block { statements, .. } => {
                self.scopes.push(HashMap::new());
                for statement in statements {
                    self.statement(statement);
                }
                self.scopes.pop();
            }
            Expression::Lambda {
                parameters, body, ..
            } => self.in_function(parameters, body),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.expression(function);
                for argument in arguments {
                    self.expression(argument);
                }
            }
            Expression::Match {
                expression, arms, ..
            } => {
                self.expression(expression);
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    self.pattern_bindings(&arm.pattern);
                    self.expression(&arm.body);
                    self.scopes.pop();
                }
            }
            Expression::BinaryOperator { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::FieldAccess { expression, .. }
            | Expression::Spread { expression, .. } => self.expression(expression),
            Expression::FieldAssign { target, value, .. } => {
                self.expression(target);
                self.expression(value);
            }
            Expression::Index {
                expression, index, ..
            } => {
                self.expression(expression);
                self.expression(index);
            }
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                self.expression(condition);
                self.expression(then);
                self.expression(else_);
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for element in elements {
                    self.expression(element);
                }
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    self.expression(key);
                    self.expression(value);
                }
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, value) in fields {
                    self.expression(value);
                }
            }
            Expression::Range { start, end, .. } => {
                self.expression(start);
                self.expression(end);
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let InterpolationPart::Hole(hole) = part {
                        self.expression(hole);
                    }
                }
            }
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. } => {}
        }
    }

    /// Bind every name `pattern` introduces into the current scope.
    fn pattern_bindings(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, span } => self.bind(name, span),
            Pattern::Constructor { arguments, .. } => {
                for argument in arguments {
                    self.pattern_bindings(argument);
                }
            }
            Pattern::Number { .. } | Pattern::Wildcard { .. } => {}
        }
    }
}

fn covers(span: &Span, offset: u32) -> bool {
    span.file == ROOT_FILE && span.start <= offset && offset < span.end
}

/// The span of the declaration binding the identifier at byte `offset` in the root
/// document, resolved against the import-linked `program` — so a name an import supplies
/// resolves to its declaration in the imported module's own file. `None` when the offset
/// is not on a resolvable identifier.
pub fn definition_at(program: &Program, offset: u32) -> Option<Span> {
    let resolver = Resolver::walk(program);
    resolver
        .references
        .iter()
        .find(|reference| covers(&reference.use_span, offset))
        .map(|reference| reference.declaration.span.clone())
}

/// The declaration's own name token: the first `Ident` token in `tokens` with the
/// declaration's name whose start lies within the declaration's span. A parameter's span
/// starts at its name (so this is trivially the first token); a top-level item's may open
/// on `>>`, which is not an `Ident`, so the search still lands on the name that follows.
fn name_token_span(tokens: &[Token], declaration: &Declaration) -> Option<Span> {
    tokens
        .iter()
        .find(|token| {
            token.kind == TokenKind::Ident
                && token.text == declaration.name
                && covers(&declaration.span, token.span.start)
        })
        .map(|token| token.span.clone())
}

/// Every span naming the same binding as the one at byte `offset` in `text`: the
/// declaration's own name token(s) — every member of an overload set, for a top-level
/// name — plus every reference resolving to it. Document-scoped: a local's references
/// never leave the document, and a top-level name's declaration must live in this
/// document too. `None` when `offset` is on nothing resolvable, on the receiver `it`
/// (which has no name token of its own), or on a name declared in another file.
pub fn references_at(program: &Program, text: &str, offset: u32) -> Option<Vec<Span>> {
    let resolver = Resolver::walk(program);
    let tokens = Lexer::tokenize(text).ok()?;

    let target = resolver
        .references
        .iter()
        .find(|reference| covers(&reference.use_span, offset))
        .map(|reference| reference.declaration.clone())
        .or_else(|| {
            resolver
                .declarations
                .iter()
                .find(|declaration| {
                    name_token_span(&tokens, declaration).is_some_and(|span| covers(&span, offset))
                })
                .cloned()
        })?;

    if target.name == RECEIVER || target.span.file != ROOT_FILE {
        return None;
    }

    let matches_target = |declaration: &Declaration| match target.is_local {
        true => declaration.span == target.span,
        false => !declaration.is_local && declaration.name == target.name,
    };

    let mut spans: Vec<Span> = resolver
        .declarations
        .iter()
        .filter(|declaration| matches_target(declaration))
        .filter_map(|declaration| name_token_span(&tokens, declaration))
        .chain(
            resolver
                .references
                .iter()
                .filter(|reference| matches_target(&reference.declaration))
                .map(|reference| reference.use_span.clone()),
        )
        .collect();
    spans.sort_by_key(|span| span.start);
    spans.dedup();
    Some(spans)
}

/// Whether `text` is a single bare name: lexes to exactly one `Ident` token and nothing
/// else — the only new name [`references_at`]'s caller may rename a binding to.
pub fn is_identifier(text: &str) -> bool {
    let Ok(tokens) = Lexer::tokenize(text) else {
        return false;
    };
    // `tokenize` always ends the stream with an `Eof` token; a single name is that plus
    // exactly one `Ident` ahead of it.
    matches!(tokens.as_slice(), [token, eof] if token.kind == TokenKind::Ident && eof.kind == TokenKind::Eof)
}

// --- Hover ------------------------------------------------------------------

/// The inferred type of the smallest expression covering byte `offset` in the root
/// document, as its display label, together with that expression's span.
pub fn hover_at(types: &TypeTable, offset: u32) -> Option<(String, Span)> {
    types
        .iter()
        .filter(|(span, _)| span.file == ROOT_FILE && span.start <= offset && offset < span.end)
        .min_by_key(|(span, _)| (span.end - span.start, span.start))
        .map(|(span, ty)| (type_label(ty), span.clone()))
}

// --- Semantic tokens --------------------------------------------------------

/// What a classified token is, in the language's own vocabulary. The server maps each
/// variant onto the protocol's legend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticTokenKind {
    /// A `<` or `>` opening or closing a block — NOT a comparison operator, which is the
    /// distinction a context-free grammar cannot make.
    BlockDelimiter,
    /// A `<` or `>` that IS the comparison operator.
    ComparisonOperator,
    TypeName,
    FunctionName,
    ParameterName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedToken {
    pub span: Span,
    pub kind: SemanticTokenKind,
}

/// Classify `text`'s tokens for semantic highlighting, in document order.
///
/// The `<` / `>` classification restates what the compiler already decided: the lexer
/// itself distinguishes a closing `>` from greater-than, and an opening `<` is a
/// comparison exactly when it follows a completed operand, which is the parser's
/// less-than recovery rule. Identifier classification names what the document's own parse
/// declares: type names, function names, and parameter names, matched by name wherever
/// the identifier appears.
pub fn semantic_tokens(text: &str) -> Vec<ClassifiedToken> {
    let Ok(tokens) = Lexer::tokenize(text) else {
        return Vec::new();
    };
    let names = DeclaredNames::of(parse_text(text).as_ref());

    let mut classified = Vec::new();
    let mut previous: Option<&TokenKind> = None;
    for token in &tokens {
        let kind = match &token.kind {
            TokenKind::BlockOpen => Some(match previous.is_some_and(ends_operand) {
                true => SemanticTokenKind::ComparisonOperator,
                false => SemanticTokenKind::BlockDelimiter,
            }),
            TokenKind::BlockClose => Some(SemanticTokenKind::BlockDelimiter),
            TokenKind::Gt => Some(SemanticTokenKind::ComparisonOperator),
            TokenKind::Ident => names.classify(&token.text),
            _ => None,
        };
        if let Some(kind) = kind {
            classified.push(ClassifiedToken {
                span: token.span.clone(),
                kind,
            });
        }
        previous = Some(&token.kind);
    }
    classified
}

/// Whether a token can END an operand — which is what makes a `<` after it the comparison
/// operator rather than a block opener (the parser's recovery rule, restated over the
/// token stream).
fn ends_operand(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::Number(_)
            | TokenKind::String(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Unit
            | TokenKind::ParenClose
            | TokenKind::BracketClose
    )
}

/// The names the document's own parse declares, bucketed by what they name. Matching is
/// by name alone, not by scope — a parameter's name colors as a parameter wherever it
/// appears in the document.
#[derive(Default)]
struct DeclaredNames {
    types: HashSet<String>,
    functions: HashSet<String>,
    parameters: HashSet<String>,
}

impl DeclaredNames {
    fn of(program: Option<&Program>) -> Self {
        let mut names = Self::default();
        let Some(program) = program else {
            return names;
        };
        for item in &program.items {
            names.collect_item(item);
        }
        for block in &program.test_blocks {
            names.collect_expression(block);
        }
        names
    }

    fn collect_item(&mut self, item: &Item) {
        match item {
            Item::TypeDeclaration(declaration) => {
                self.types.insert(declaration.name.clone());
                for method in declaration.type_definition.methods() {
                    self.functions.insert(method.name.clone());
                    self.collect_parameters(&method.parameters);
                    self.collect_expression(&method.body);
                }
            }
            Item::FunctionDeclaration(declaration) => {
                self.functions.insert(declaration.name.clone());
                self.collect_parameters(&declaration.parameters);
                self.collect_expression(&declaration.body);
            }
            Item::VariableDeclaration(declaration) => self.collect_expression(&declaration.value),
        }
    }

    fn collect_parameters(&mut self, parameters: &[Parameter]) {
        for parameter in parameters {
            self.parameters.insert(parameter.name.clone());
        }
    }

    fn collect_expression(&mut self, expression: &Expression) {
        walk_expressions(expression, &mut |node| match node {
            Expression::Lambda { parameters, .. } => self.collect_parameters(parameters),
            Expression::Block { statements, .. } => {
                for statement in statements {
                    if let Statement::Item(item) = statement {
                        self.collect_item_shallow(item);
                    }
                }
            }
            _ => {}
        });
    }

    /// A block-local declaration: record its name in the right bucket (its body is walked
    /// by the surrounding expression walk already).
    fn collect_item_shallow(&mut self, item: &Item) {
        match item {
            Item::TypeDeclaration(declaration) => {
                self.types.insert(declaration.name.clone());
            }
            Item::FunctionDeclaration(declaration) => {
                self.functions.insert(declaration.name.clone());
                self.collect_parameters(&declaration.parameters);
            }
            Item::VariableDeclaration(_) => {}
        }
    }

    fn classify(&self, name: &str) -> Option<SemanticTokenKind> {
        if self.types.contains(name) {
            return Some(SemanticTokenKind::TypeName);
        }
        if self.functions.contains(name) {
            return Some(SemanticTokenKind::FunctionName);
        }
        if self.parameters.contains(name) {
            return Some(SemanticTokenKind::ParameterName);
        }
        None
    }
}

// --- Test lenses ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestLensKind {
    /// A `describe` block.
    Suite,
    /// An `it` case.
    Case,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestLens {
    pub kind: TestLensKind,
    /// The block's description — its first argument when that is a string literal.
    pub name: String,
    /// The `/`-joined path from the outermost enclosing `describe` down to this suite or
    /// case — the same path `quilon test --only` expects (see
    /// `docs/corelib/test/README.md#paths`).
    pub path: String,
    /// The span of the whole `describe(...)` / `it(...)` call.
    pub span: Span,
}

/// Every test suite and case in the document's own parse, in document order: each
/// top-level `describe` block and, inside the blocks, every nested `describe` and every
/// `it` call. Empty when the document does not parse or has no test blocks.
///
/// Mirrors `test_command::collect_paths`'s path-building (the names from the outermost
/// `describe` down, joined by `/`), but walks the document's OWN pre-link parse rather
/// than the linked, qualified program: lenses and semantic tokens both speak about the
/// text as written.
pub fn test_lenses(text: &str) -> Vec<TestLens> {
    let Some(program) = parse_text(text) else {
        return Vec::new();
    };
    let mut lenses = Vec::new();
    for block in &program.test_blocks {
        collect_test_lenses(block, "", &mut lenses);
    }
    lenses
}

fn collect_test_lenses(expression: &Expression, prefix: &str, lenses: &mut Vec<TestLens>) {
    match expression {
        Expression::Call {
            function,
            arguments,
            member_call: false,
            span,
        } => {
            if let Some(kind) = harness_call_kind(function)
                && let [Expression::String { value, .. }, body] = arguments.as_slice()
            {
                let path = match prefix.is_empty() {
                    true => value.clone(),
                    false => format!("{prefix}/{value}"),
                };
                lenses.push(TestLens {
                    kind,
                    name: value.clone(),
                    path: path.clone(),
                    span: span.clone(),
                });
                if kind == TestLensKind::Suite {
                    collect_test_lenses(body, &path, lenses);
                }
                return;
            }
            collect_test_lenses(function, prefix, lenses);
            for argument in arguments {
                collect_test_lenses(argument, prefix, lenses);
            }
        }
        Expression::Lambda { body, .. } => collect_test_lenses(body, prefix, lenses),
        Expression::Block { statements, .. } => {
            for statement in statements {
                if let Statement::Expression(nested) = statement {
                    collect_test_lenses(nested, prefix, lenses);
                }
            }
        }
        _ => {}
    }
}

/// Whether `function` is a module-qualified call of the harness's `describe` or `it`
/// (`test.describe`, `core.test.it`) — the pre-link spelling.
fn harness_call_kind(function: &Expression) -> Option<TestLensKind> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    if !name.contains('.') {
        return None;
    }
    match display_name(name) {
        "describe" => Some(TestLensKind::Suite),
        "it" => Some(TestLensKind::Case),
        _ => None,
    }
}

// --- Shared expression walk -------------------------------------------------

/// Call `visit` on `expression` and every expression nested anywhere inside it, in
/// document order.
fn walk_expressions<'a>(expression: &'a Expression, visit: &mut impl FnMut(&'a Expression)) {
    visit(expression);
    match expression {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            walk_expressions(function, visit);
            for argument in arguments {
                walk_expressions(argument, visit);
            }
        }
        Expression::Lambda { body, .. } => walk_expressions(body, visit),
        Expression::Block { statements, .. } => {
            for statement in statements {
                match statement {
                    Statement::Expression(nested) => walk_expressions(nested, visit),
                    Statement::Item(item) => match item {
                        Item::VariableDeclaration(declaration) => {
                            walk_expressions(&declaration.value, visit)
                        }
                        Item::FunctionDeclaration(declaration) => {
                            walk_expressions(&declaration.body, visit)
                        }
                        Item::TypeDeclaration(declaration) => {
                            for method in declaration.type_definition.methods() {
                                walk_expressions(&method.body, visit);
                            }
                        }
                    },
                }
            }
        }
        Expression::BinaryOperator { left, right, .. } => {
            walk_expressions(left, visit);
            walk_expressions(right, visit);
        }
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. }
        | Expression::Spread { expression, .. } => walk_expressions(expression, visit),
        Expression::FieldAssign { target, value, .. } => {
            walk_expressions(target, visit);
            walk_expressions(value, visit);
        }
        Expression::Index {
            expression, index, ..
        } => {
            walk_expressions(expression, visit);
            walk_expressions(index, visit);
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            walk_expressions(condition, visit);
            walk_expressions(then, visit);
            walk_expressions(else_, visit);
        }
        Expression::Match {
            expression, arms, ..
        } => {
            walk_expressions(expression, visit);
            for arm in arms {
                walk_expressions(&arm.body, visit);
            }
        }
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            for element in elements {
                walk_expressions(element, visit);
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                walk_expressions(key, visit);
                walk_expressions(value, visit);
            }
        }
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            for (_, value) in fields {
                walk_expressions(value, visit);
            }
        }
        Expression::Range { start, end, .. } => {
            walk_expressions(start, visit);
            walk_expressions(end, visit);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Hole(hole) = part {
                    walk_expressions(hole, visit);
                }
            }
        }
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. }
        | Expression::Identifier { .. } => {}
    }
}
