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
    Expression, FunctionDeclaration, Import, InterpolationPart, Item, MethodDeclaration,
    ModulePath, Parameter, Pattern, Program, RECEIVER, Statement, SumVariant, Type,
    TypeDeclaration, TypeDefinition, display_name, type_label,
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

/// The name and span of the declaration binding the identifier at byte `offset` in the
/// root document, resolved against the import-linked `program` — so a name an import
/// supplies resolves to its declaration in the imported module's own file. `None` when
/// the offset is not on a resolvable identifier.
pub fn declaration_at(program: &Program, offset: u32) -> Option<(String, Span)> {
    Resolver::walk(program)
        .references
        .into_iter()
        .find(|reference| covers(&reference.use_span, offset))
        .map(|reference| (reference.declaration.name, reference.declaration.span))
}

/// The span alone of [`declaration_at`]'s answer — what go-to-definition needs.
pub fn definition_at(program: &Program, offset: u32) -> Option<Span> {
    declaration_at(program, offset).map(|(_, span)| span)
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

// --- Completion ---------------------------------------------------------------

/// Maps onto the protocol's `CompletionItemKind` in `src/lsp.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Function,
    Field,
    Method,
    Class,
    Module,
    EnumMember,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// The type or signature, in Quilon spelling — [`type_label`], the same formatter
    /// hover uses.
    pub detail: Option<String>,
}

/// Complete at byte offset `offset` in `text`, the buffer at `path`.
///
/// The document at the cursor is normally unparseable (`response.` has no member yet), so
/// every request deletes just the incomplete token at the cursor — the trailing `.member`
/// or the bare word being typed — before parsing: what's left parses like the document one
/// token earlier. A completion request whose cursor sits in an expression broken for an
/// unrelated reason answers empty regardless.
pub fn completions_at(path: &Path, text: &str, offset: u32) -> Vec<CompletionItem> {
    let offset = offset as usize;
    match dot_before_cursor(text, offset) {
        Some(dot) => member_completions(path, text, dot, offset),
        None => scope_completions_at(text, offset),
    }
}

fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `text` with the byte range `[start, end)` deleted.
fn without(text: &str, start: usize, end: usize) -> String {
    let mut out = String::with_capacity(text.len() - (end - start));
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    out
}

/// The byte offset just before `offset` while scanning back over identifier characters —
/// the start of the (possibly empty) word being typed there.
fn word_start_before(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = offset;
    while i > 0 && is_ident_char(bytes[i - 1]) {
        i -= 1;
    }
    i
}

/// The byte offset of the `.` immediately before `offset`, once any partial member name
/// being typed there is skipped — `None` when `offset` is not in a member-access position
/// at all.
fn dot_before_cursor(text: &str, offset: usize) -> Option<usize> {
    let word_start = word_start_before(text, offset);
    let bytes = text.as_bytes();
    (word_start > 0 && bytes[word_start - 1] == b'.').then(|| word_start - 1)
}

/// The bare identifier ending exactly at byte `dot` in `text`, when there is one — `None`
/// when `dot` is not immediately preceded by a name, or that name is itself part of a
/// longer `.`-chain (`x.http.` — `http` there is a field of `x`, not an import binding).
fn bare_identifier_ending_at(text: &str, dot: usize) -> Option<&str> {
    let start = word_start_before(text, dot);
    if start == dot || (start > 0 && text.as_bytes()[start - 1] == b'.') {
        return None;
    }
    Some(&text[start..dot])
}

/// Case 1: names in scope at `offset`. Purely syntactic — no type-check — so this works
/// on a document [`check_text`] would reject.
fn scope_completions_at(text: &str, offset: usize) -> Vec<CompletionItem> {
    let word_start = word_start_before(text, offset);
    let stripped = without(text, word_start, offset);
    let Some(program) = parse_text(&stripped) else {
        return Vec::new();
    };
    let offset = word_start as u32;

    let mut scopes: Vec<HashMap<String, CompletionItem>> = vec![HashMap::new()];
    let mut sums: Vec<CompletionItem> = Vec::new();
    walk_items(&program.items, offset, &mut scopes, &mut sums);
    for block in &program.test_blocks {
        if covers(block.span(), offset) {
            scope_walk(block, offset, &mut scopes, &mut sums);
        }
    }

    let mut merged: HashMap<String, CompletionItem> = HashMap::new();
    for scope in scopes {
        merged.extend(scope);
    }
    let mut items: Vec<CompletionItem> = merged.into_values().collect();
    items.extend(sums);
    for import in &program.imports {
        if let Some(binding) = import.path.binding_name() {
            items.push(CompletionItem {
                label: binding,
                kind: CompletionKind::Module,
                detail: Some(module_path_label(&import.path)),
            });
        }
    }
    items
}

fn module_path_label(path: &ModulePath) -> String {
    match path {
        ModulePath::BuiltinDotted(parts) => parts.join("."),
        ModulePath::FilePath(raw) => raw.clone(),
    }
}

/// Walk a program's top-level items up to `offset` (see [`item_step`]).
fn walk_items(
    items: &[Item],
    offset: u32,
    scopes: &mut Vec<HashMap<String, CompletionItem>>,
    sums: &mut Vec<CompletionItem>,
) {
    for item in items {
        if item_step(item, offset, scopes, sums) {
            return;
        }
    }
}

/// One item's contribution to a positioned search over a list of items in document order.
/// Returns whether the caller's loop should stop. Shared by [`walk_items`] and
/// [`scope_walk`]'s `Block` arm — the two places a list of items is walked this way.
fn item_step(
    item: &Item,
    offset: u32,
    scopes: &mut Vec<HashMap<String, CompletionItem>>,
    sums: &mut Vec<CompletionItem>,
) -> bool {
    if item.span().start > offset {
        return true;
    }
    if covers(item.span(), offset) {
        if let Item::TypeDeclaration(declaration) = item {
            collect_sum_constructors(declaration, sums);
        }
        descend_item_body(item, offset, scopes, sums);
        return true;
    }
    bind_item(item, scopes, sums);
    false
}

fn bind_item(
    item: &Item,
    scopes: &mut [HashMap<String, CompletionItem>],
    sums: &mut Vec<CompletionItem>,
) {
    if let Item::TypeDeclaration(declaration) = item {
        collect_sum_constructors(declaration, sums);
    }
    let completion = match item {
        Item::FunctionDeclaration(declaration) => CompletionItem {
            label: declaration.name.clone(),
            kind: CompletionKind::Function,
            detail: Some(function_signature_label(declaration)),
        },
        Item::TypeDeclaration(declaration) => CompletionItem {
            label: declaration.name.clone(),
            kind: CompletionKind::Class,
            detail: None,
        },
        Item::VariableDeclaration(declaration) => CompletionItem {
            label: declaration.name.clone(),
            kind: CompletionKind::Variable,
            detail: declaration.type_annotation.as_ref().map(type_label),
        },
    };
    if let Some(scope) = scopes.last_mut() {
        scope.insert(completion.label.clone(), completion);
    }
}

fn collect_sum_constructors(declaration: &TypeDeclaration, sums: &mut Vec<CompletionItem>) {
    if let TypeDefinition::Sum { variants, .. } = &declaration.type_definition {
        for variant in variants {
            sums.push(CompletionItem {
                label: variant.name.clone(),
                kind: CompletionKind::EnumMember,
                detail: Some(declaration.name.clone()),
            });
        }
    }
}

fn bind_parameter(parameter: &Parameter, scopes: &mut [HashMap<String, CompletionItem>]) {
    let completion = CompletionItem {
        label: parameter.name.clone(),
        kind: CompletionKind::Variable,
        detail: parameter.type_annotation.as_ref().map(type_label),
    };
    if let Some(scope) = scopes.last_mut() {
        scope.insert(completion.label.clone(), completion);
    }
}

fn descend_item_body(
    item: &Item,
    offset: u32,
    scopes: &mut Vec<HashMap<String, CompletionItem>>,
    sums: &mut Vec<CompletionItem>,
) {
    match item {
        Item::FunctionDeclaration(declaration) => {
            scopes.push(HashMap::new());
            for parameter in &declaration.parameters {
                bind_parameter(parameter, scopes);
            }
            scope_walk(&declaration.body, offset, scopes, sums);
        }
        Item::TypeDeclaration(declaration) => {
            for method in declaration.type_definition.methods() {
                if covers(&method.span, offset) {
                    scopes.push(HashMap::new());
                    if let Some(scope) = scopes.last_mut() {
                        scope.insert(
                            RECEIVER.to_string(),
                            CompletionItem {
                                label: RECEIVER.to_string(),
                                kind: CompletionKind::Variable,
                                detail: Some(declaration.name.clone()),
                            },
                        );
                    }
                    for parameter in &method.parameters {
                        bind_parameter(parameter, scopes);
                    }
                    scope_walk(&method.body, offset, scopes, sums);
                    return;
                }
            }
        }
        Item::VariableDeclaration(declaration) => {
            scope_walk(&declaration.value, offset, scopes, sums);
        }
    }
}

/// Find the child (if any) whose span covers `offset`, recurse, and along the way push a
/// new scope for a `Block`'s statements, a `Lambda`'s parameters, or a matched `Match`
/// arm's pattern bindings.
fn scope_walk(
    expression: &Expression,
    offset: u32,
    scopes: &mut Vec<HashMap<String, CompletionItem>>,
    sums: &mut Vec<CompletionItem>,
) {
    if !covers(expression.span(), offset) {
        return;
    }
    match expression {
        Expression::Block { statements, .. } => {
            scopes.push(HashMap::new());
            for statement in statements {
                match statement {
                    Statement::Item(item) => {
                        if item_step(item, offset, scopes, sums) {
                            return;
                        }
                    }
                    Statement::Expression(nested) => {
                        if nested.span().start > offset {
                            return;
                        }
                        if covers(nested.span(), offset) {
                            scope_walk(nested, offset, scopes, sums);
                            return;
                        }
                    }
                }
            }
        }
        Expression::Lambda {
            parameters, body, ..
        } => {
            scopes.push(HashMap::new());
            for parameter in parameters {
                bind_parameter(parameter, scopes);
            }
            scope_walk(body, offset, scopes, sums);
        }
        Expression::Match {
            expression: scrutinee,
            arms,
            ..
        } => {
            if covers(scrutinee.span(), offset) {
                scope_walk(scrutinee, offset, scopes, sums);
                return;
            }
            for arm in arms {
                if covers(&arm.span, offset) {
                    scopes.push(HashMap::new());
                    bind_pattern(&arm.pattern, scopes);
                    scope_walk(&arm.body, offset, scopes, sums);
                    return;
                }
            }
        }
        // Every other composite form has at most one child covering `offset` (spans nest,
        // never overlap); [`expression_statement_span`] shares this same child list.
        other => {
            for child in expression_children(other) {
                if covers(child.span(), offset) {
                    scope_walk(child, offset, scopes, sums);
                    return;
                }
            }
        }
    }
}

/// The immediate children of every "pass-through" expression form — one that introduces
/// no binding or statement boundary of its own. `Block`/`Lambda`/`Match` each DO introduce
/// one, so [`scope_walk`] and [`expression_statement_span`] special-case those three
/// themselves and share this list for everything else.
fn expression_children(expression: &Expression) -> Vec<&Expression> {
    match expression {
        Expression::Call {
            function,
            arguments,
            ..
        } => std::iter::once(function.as_ref())
            .chain(arguments.iter())
            .collect(),
        Expression::BinaryOperator { left, right, .. } => vec![left, right],
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. }
        | Expression::Spread { expression, .. } => vec![expression],
        Expression::FieldAssign { target, value, .. } => vec![target, value],
        Expression::Index {
            expression, index, ..
        } => vec![expression, index],
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => vec![condition, then, else_],
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            elements.iter().collect()
        }
        Expression::MapLiteral { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [key, value])
            .collect(),
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            fields.iter().map(|(_, value)| value).collect()
        }
        Expression::Range { start, end, .. } => vec![start, end],
        Expression::Interpolation { parts, .. } => parts
            .iter()
            .filter_map(|part| match part {
                InterpolationPart::Hole(hole) => Some(hole),
                InterpolationPart::Literal(_) => None,
            })
            .collect(),
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. }
        | Expression::Identifier { .. }
        | Expression::Block { .. }
        | Expression::Lambda { .. }
        | Expression::Match { .. } => Vec::new(),
    }
}

fn bind_pattern(pattern: &Pattern, scopes: &mut [HashMap<String, CompletionItem>]) {
    match pattern {
        Pattern::Identifier { name, .. } => {
            let completion = CompletionItem {
                label: name.clone(),
                kind: CompletionKind::Variable,
                detail: None,
            };
            if let Some(scope) = scopes.last_mut() {
                scope.insert(name.clone(), completion);
            }
        }
        Pattern::Constructor { arguments, .. } => {
            for argument in arguments {
                bind_pattern(argument, scopes);
            }
        }
        Pattern::Number { .. } | Pattern::Wildcard { .. } => {}
    }
}

/// A function's call signature, in Quilon spelling. `declared_return_type`/
/// `parameter_type` read both annotation forms (per-parameter, or a whole-signature `::`).
fn function_signature_label(declaration: &FunctionDeclaration) -> String {
    let parameters: Vec<String> = (0..declaration.parameters.len())
        .map(|index| {
            declaration
                .parameter_type(index)
                .map(type_label)
                .unwrap_or_else(|| "_".to_string())
        })
        .collect();
    let ret = declaration
        .declared_return_type()
        .map(type_label)
        .unwrap_or_else(|| "_".to_string());
    format!("({}) -> {ret}", parameters.join(", "))
}

/// Case 2/3: complete after a `.` at byte `dot`, deleting it and any partial member name
/// already typed. Case 3 isolates the receiver from its enclosing STATEMENT before
/// checking — cutting everything else that statement covers, both before the receiver
/// (`1 + `) and after it (a call's closing `)`) — so a genuine mismatch elsewhere in the
/// statement (`1 + s.` strips to `1 + s`, `Num + Text`) doesn't blank the result. This is a
/// no-op when the receiver is already the whole statement, so it needs no separate
/// simpler-case attempt.
fn member_completions(path: &Path, text: &str, dot: usize, offset: usize) -> Vec<CompletionItem> {
    let stripped = without(text, dot, offset);
    let Some(program) = parse_text(&stripped) else {
        return Vec::new();
    };

    if let Some(binding) = bare_identifier_ending_at(text, dot)
        && let Some(import) = program
            .imports
            .iter()
            .find(|import| import.path.binding_name().as_deref() == Some(binding))
    {
        return module_completions(path, import);
    }

    let Some(receiver) = expression_ending_at(&program, dot as u32) else {
        return Vec::new();
    };
    let Some(statement) = statement_span_at(&program, dot as u32) else {
        return Vec::new();
    };
    if statement.start > receiver.start || statement.end < dot as u32 {
        return Vec::new();
    }
    // Cut the suffix FIRST (it lies entirely at or after `dot`), so the prefix cut right
    // after — entirely before `dot` — still lands on the same byte offsets.
    let without_suffix = without(&stripped, dot, statement.end as usize);
    let isolated = without(
        &without_suffix,
        statement.start as usize,
        receiver.start as usize,
    );
    let new_dot = dot - (receiver.start as usize - statement.start as usize);
    let Ok(checked) = check_text(path, &isolated) else {
        return Vec::new();
    };
    let Some(ty) = type_ending_at(&checked.types, &isolated, new_dot) else {
        return Vec::new();
    };
    type_member_completions(&checked.program, &checked.types, ty)
}

/// The type of the smallest expression ending exactly at byte `end` in `types`. Skips
/// back over spaces/tabs first, so a deliberately spaced member access (`response .body`)
/// still resolves.
fn type_ending_at<'a>(types: &'a TypeTable, text: &str, end: usize) -> Option<&'a Type> {
    let bytes = text.as_bytes();
    let mut end = end;
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let end = end as u32;
    types
        .iter()
        .filter(|(span, _)| span.file == ROOT_FILE && span.end == end)
        .min_by_key(|(span, _)| span.end - span.start)
        .map(|(_, ty)| ty)
}

/// Case 2: `import`'s exported members, resolved by loading it in isolation
/// ([`crate::modules::link`]) — a bare import binding is a qualifier, not a checkable
/// expression, so this never touches the rest of the document.
fn module_completions(path: &Path, import: &Import) -> Vec<CompletionItem> {
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let synthetic = Program {
        imports: vec![import.clone()],
        items: Vec::new(),
        test_blocks: Vec::new(),
    };
    let Ok((linked, _sources)) = crate::modules::link(synthetic, base_dir, Some(path)) else {
        return Vec::new();
    };
    let Some(canonical) = canonical_prefix(&import.path) else {
        return Vec::new();
    };
    let prefix = format!("{canonical}.");

    let mut items = Vec::new();
    for item in &linked.items {
        if !crate::modules::item_is_exported(item) || item.name().starts_with('@') {
            continue;
        }
        if let Some(short) = item.name().strip_prefix(&prefix) {
            items.push(module_completion_item(item, short));
        }
        if let Item::TypeDeclaration(declaration) = item
            && declaration.name.starts_with(&prefix)
            && let TypeDefinition::Sum { variants, .. } = &declaration.type_definition
        {
            items.extend(module_sum_variant_items(
                variants,
                &prefix,
                &declaration.name,
            ));
        }
    }
    items
}

fn module_sum_variant_items(
    variants: &[SumVariant],
    prefix: &str,
    type_name: &str,
) -> Vec<CompletionItem> {
    variants
        .iter()
        .filter_map(|variant| variant.name.strip_prefix(prefix))
        .map(|short| CompletionItem {
            label: short.to_string(),
            kind: CompletionKind::EnumMember,
            detail: Some(display_name(type_name).to_string()),
        })
        .collect()
}

/// Matches `src/modules.rs`'s own choice of canonical name per import form.
fn canonical_prefix(path: &ModulePath) -> Option<String> {
    match path {
        ModulePath::BuiltinDotted(parts) => Some(parts.join(".")),
        ModulePath::FilePath(_) => path.binding_name(),
    }
}

fn module_completion_item(item: &Item, short: &str) -> CompletionItem {
    match item {
        Item::FunctionDeclaration(declaration) => CompletionItem {
            label: short.to_string(),
            kind: CompletionKind::Function,
            detail: Some(function_signature_label(declaration)),
        },
        Item::TypeDeclaration(_) => CompletionItem {
            label: short.to_string(),
            kind: CompletionKind::Class,
            detail: None,
        },
        Item::VariableDeclaration(declaration) => CompletionItem {
            label: short.to_string(),
            kind: CompletionKind::Variable,
            detail: declaration.type_annotation.as_ref().map(type_label),
        },
    }
}

/// Case 3: the members of `ty` reached through `.`.
fn type_member_completions(program: &Program, types: &TypeTable, ty: &Type) -> Vec<CompletionItem> {
    match ty {
        Type::Record(fields) => fields
            .iter()
            .map(|(name, field_type)| CompletionItem {
                label: name.clone(),
                kind: CompletionKind::Field,
                detail: Some(type_label(field_type)),
            })
            .collect(),
        Type::Named { name, fields, .. } => {
            let mut items: Vec<CompletionItem> = fields
                .iter()
                .map(|(name, field_type)| CompletionItem {
                    label: name.clone(),
                    kind: CompletionKind::Field,
                    detail: Some(type_label(field_type)),
                })
                .collect();
            items.extend(user_method_completions(program, types, name));
            items
        }
        Type::Sum { name, .. } => user_method_completions(program, types, name),
        Type::Text => text_method_completions(),
        Type::Array(elem) => array_method_completions(elem),
        Type::Map(key, value) => map_method_completions(key, value),
        Type::Set(elem) => set_method_completions(elem),
        _ => Vec::new(),
    }
}

/// Filters out operator and render (`` ` ``) members via [`is_identifier`] — neither is
/// ever typed after a `.`.
fn user_method_completions(
    program: &Program,
    types: &TypeTable,
    type_name: &str,
) -> Vec<CompletionItem> {
    let Some(declaration) = find_type_declaration(program, type_name) else {
        return Vec::new();
    };
    declaration
        .type_definition
        .methods()
        .iter()
        .filter(|method| is_identifier(&method.name))
        .map(|method| CompletionItem {
            label: method.name.clone(),
            kind: CompletionKind::Method,
            detail: Some(method_signature_label(method, types)),
        })
        .collect()
}

/// The bodies (or method bodies) an item carries.
fn item_bodies(item: &Item) -> Vec<&Expression> {
    match item {
        Item::FunctionDeclaration(declaration) => vec![&declaration.body],
        Item::VariableDeclaration(declaration) => vec![&declaration.value],
        Item::TypeDeclaration(declaration) => declaration
            .type_definition
            .methods()
            .iter()
            .map(|method| &method.body)
            .collect(),
    }
}

/// `type_name`'s own [`TypeDeclaration`], searched top-level first, then anywhere nested
/// inside a block (a type may be declared locally too).
fn find_type_declaration<'a>(program: &'a Program, type_name: &str) -> Option<&'a TypeDeclaration> {
    for item in &program.items {
        if let Item::TypeDeclaration(declaration) = item
            && declaration.name == type_name
        {
            return Some(declaration);
        }
    }
    program
        .items
        .iter()
        .find_map(|item| find_type_declaration_in_item(item, type_name))
}

fn find_type_declaration_in_item<'a>(
    item: &'a Item,
    type_name: &str,
) -> Option<&'a TypeDeclaration> {
    item_bodies(item)
        .into_iter()
        .find_map(|body| find_type_declaration_in_expression(body, type_name))
}

fn find_type_declaration_in_expression<'a>(
    expression: &'a Expression,
    type_name: &str,
) -> Option<&'a TypeDeclaration> {
    let mut found = None;
    walk_expressions(expression, &mut |node| {
        if found.is_some() {
            return;
        }
        let Expression::Block { statements, .. } = node else {
            return;
        };
        for statement in statements {
            let Statement::Item(item) = statement else {
                continue;
            };
            if let Item::TypeDeclaration(declaration) = item
                && declaration.name == type_name
            {
                found = Some(declaration);
                return;
            }
            if let Some(inner) = find_type_declaration_in_item(item, type_name) {
                found = Some(inner);
                return;
            }
        }
    });
    found
}

/// A syntax-only analogue of [`type_ending_at`], for before the document has checked at
/// all (there is no [`TypeTable`] yet to read a span out of).
fn expression_ending_at(program: &Program, end: u32) -> Option<Span> {
    let mut best: Option<Span> = None;
    let mut consider = |expression: &Expression| {
        let span = expression.span();
        if span.end == end
            && best
                .as_ref()
                .is_none_or(|current| span.end - span.start < current.end - current.start)
        {
            best = Some(span.clone());
        }
    };
    for item in &program.items {
        for body in item_bodies(item) {
            walk_expressions(body, &mut consider);
        }
    }
    for block in &program.test_blocks {
        walk_expressions(block, &mut consider);
    }
    best
}

/// [`covers`], but INCLUSIVE of a span's own end: `offset` here is always the byte right
/// after a receiver's trailing `.member` was cut off, i.e. exactly the receiver's own end,
/// which strict `covers` would call past the span.
fn ends_at_or_covers(span: &Span, offset: u32) -> bool {
    span.file == ROOT_FILE && span.start <= offset && offset <= span.end
}

/// The span of the smallest STATEMENT — a block's own statement, or a top-level item's
/// whole declaration — covering byte `offset`.
fn statement_span_at(program: &Program, offset: u32) -> Option<Span> {
    program
        .items
        .iter()
        .find(|item| ends_at_or_covers(item.span(), offset))
        .map(|item| item_statement_span(item, offset))
}

/// `item`'s own declaration span, unless `offset` sits inside a block nested somewhere in
/// its body — then that block's own covering statement's span.
fn item_statement_span(item: &Item, offset: u32) -> Span {
    item_bodies(item)
        .into_iter()
        .filter(|body| ends_at_or_covers(body.span(), offset))
        .find_map(|body| expression_statement_span(body, offset))
        .unwrap_or_else(|| item.span().clone())
}

/// `Some(span)` when `offset` is inside a `Block` nested somewhere in `expression`; `None`
/// when it carries no block at all covering `offset`.
fn expression_statement_span(expression: &Expression, offset: u32) -> Option<Span> {
    if !ends_at_or_covers(expression.span(), offset) {
        return None;
    }
    match expression {
        Expression::Block { statements, .. } => {
            for statement in statements {
                let (span, item) = match statement {
                    Statement::Item(item) => (item.span(), Some(item)),
                    Statement::Expression(nested) => (nested.span(), None),
                };
                if !ends_at_or_covers(span, offset) {
                    continue;
                }
                return Some(match item {
                    Some(item) => item_statement_span(item, offset),
                    None => span.clone(),
                });
            }
            None
        }
        Expression::Lambda { body, .. } => expression_statement_span(body, offset),
        Expression::Match {
            expression: scrutinee,
            arms,
            ..
        } => expression_statement_span(scrutinee, offset).or_else(|| {
            arms.iter()
                .find(|arm| ends_at_or_covers(&arm.span, offset))
                .and_then(|arm| expression_statement_span(&arm.body, offset))
        }),
        other => expression_children(other)
            .into_iter()
            .find(|child| ends_at_or_covers(child.span(), offset))
            .and_then(|child| expression_statement_span(child, offset)),
    }
}

/// A method's call signature. The return type is the annotation when present, else the
/// body's own inferred type (a self-typed return, e.g. a method returning `it`).
fn method_signature_label(method: &MethodDeclaration, types: &TypeTable) -> String {
    let parameters: Vec<String> = method
        .parameters
        .iter()
        .map(|parameter| {
            parameter
                .type_annotation
                .as_ref()
                .map(type_label)
                .unwrap_or_else(|| "_".to_string())
        })
        .collect();
    let ret = method
        .return_type
        .as_ref()
        .or_else(|| types.get(method.body.span()))
        .map(type_label)
        .unwrap_or_else(|| "_".to_string());
    format!("({}) -> {ret}", parameters.join(", "))
}

/// Mirrors the type checker's own (private) `result_of` in `src/typechecker/checker/sums.rs`.
fn result_of(elem: Type) -> Type {
    Type::Sum {
        name: "Result".to_string(),
        variants: vec![
            SumVariant {
                name: "Ok".to_string(),
                fields: vec![elem],
            },
            SumVariant {
                name: "NotOk".to_string(),
                fields: vec![Type::Unit],
            },
        ],
    }
}

fn function_type(parameters: Vec<Type>, return_type: Type) -> Type {
    Type::Function {
        parameters,
        return_type: Box::new(return_type),
    }
}

fn method_items(table: Vec<(&str, Vec<Type>, Type)>) -> Vec<CompletionItem> {
    table
        .into_iter()
        .map(|(name, parameters, ret)| CompletionItem {
            label: name.to_string(),
            kind: CompletionKind::Method,
            detail: Some(format!(
                "({}) -> {}",
                parameters
                    .iter()
                    .map(type_label)
                    .collect::<Vec<_>>()
                    .join(", "),
                type_label(&ret)
            )),
        })
        .collect()
}

fn field_item(name: &str, ty: Type) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: CompletionKind::Field,
        detail: Some(type_label(&ty)),
    }
}

/// Mirrors `TypeChecker::check_text_method`'s table in `src/typechecker/checker/calls.rs`
/// (see also `docs/types/text.md`).
fn text_method_completions() -> Vec<CompletionItem> {
    let mut items = method_items(text_method_table());
    items.push(field_item("size", Type::Num));
    items.push(field_item("length", Type::Num));
    items
}

fn text_method_table() -> Vec<(&'static str, Vec<Type>, Type)> {
    use Type::{Bool, Num, Text};
    vec![
        ("trim", vec![], Text),
        ("trimStart", vec![], Text),
        ("trimEnd", vec![], Text),
        ("toUpper", vec![], Text),
        ("toLower", vec![], Text),
        ("split", vec![Text], Type::Array(Box::new(Text))),
        ("graphemes", vec![], Type::Array(Box::new(Text))),
        ("contains", vec![Text], Bool),
        ("indexOf", vec![Text], result_of(Num)),
        ("at", vec![Num], result_of(Text)),
        ("slice", vec![Num, Num], Text),
        ("repeat", vec![Num], Text),
        ("replaceAll", vec![Text, Text], Text),
        ("replace", vec![Text, Text, Num], Text),
    ]
}

/// Mirrors `TypeChecker::check_array_method`'s signatures. `map`/`reduce`'s own result
/// type is generic, spelled with a placeholder name (`R`, `A`).
fn array_method_completions(elem: &Type) -> Vec<CompletionItem> {
    let mut items = method_items(array_method_table(elem));
    items.push(field_item("size", Type::Num));
    items
}

fn array_method_table(elem: &Type) -> Vec<(&'static str, Vec<Type>, Type)> {
    let r = Type::named_ref("R");
    let a = Type::named_ref("A");
    vec![
        (
            "map",
            vec![function_type(vec![elem.clone()], r.clone())],
            Type::Array(Box::new(r)),
        ),
        (
            "filter",
            vec![function_type(vec![elem.clone()], Type::Bool)],
            Type::Array(Box::new(elem.clone())),
        ),
        (
            "reduce",
            vec![
                a.clone(),
                function_type(vec![a.clone(), elem.clone()], a.clone()),
            ],
            a,
        ),
        (
            "each",
            vec![function_type(vec![elem.clone()], Type::Unit)],
            Type::Array(Box::new(elem.clone())),
        ),
        (
            "find",
            vec![function_type(vec![elem.clone()], Type::Bool)],
            result_of(elem.clone()),
        ),
        ("at", vec![Type::Num], result_of(elem.clone())),
    ]
}

/// Mirrors `TypeChecker::check_map_method`'s signatures.
fn map_method_completions(key: &Type, value: &Type) -> Vec<CompletionItem> {
    let mut items = method_items(map_method_table(key, value));
    items.push(field_item("size", Type::Num));
    items
}

fn map_method_table(key: &Type, value: &Type) -> Vec<(&'static str, Vec<Type>, Type)> {
    let map_type = Type::Map(Box::new(key.clone()), Box::new(value.clone()));
    vec![
        ("get", vec![key.clone()], result_of(value.clone())),
        ("has", vec![key.clone()], Type::Bool),
        ("set", vec![key.clone(), value.clone()], map_type.clone()),
        ("remove", vec![key.clone()], map_type.clone()),
        ("keys", vec![], Type::Array(Box::new(key.clone()))),
        ("values", vec![], Type::Array(Box::new(value.clone()))),
        (
            "each",
            vec![function_type(vec![key.clone(), value.clone()], Type::Unit)],
            map_type,
        ),
    ]
}

/// Mirrors `TypeChecker::check_set_method`'s signatures.
fn set_method_completions(elem: &Type) -> Vec<CompletionItem> {
    let mut items = method_items(set_method_table(elem));
    items.push(field_item("size", Type::Num));
    items
}

fn set_method_table(elem: &Type) -> Vec<(&'static str, Vec<Type>, Type)> {
    let set_type = Type::Set(Box::new(elem.clone()));
    vec![
        ("has", vec![elem.clone()], Type::Bool),
        ("add", vec![elem.clone()], set_type.clone()),
        ("remove", vec![elem.clone()], set_type.clone()),
        ("items", vec![], Type::Array(Box::new(elem.clone()))),
        (
            "each",
            vec![function_type(vec![elem.clone()], Type::Unit)],
            set_type,
        ),
    ]
}

#[cfg(test)]
mod builtin_table_tests {
    //! Guards `text_method_table`/`array_method_table`/`map_method_table`/
    //! `set_method_table` against drifting from the checker's own signatures
    //! (`src/typechecker/checker/calls.rs`): for every entry, synthesize a call, check it,
    //! and compare the checker's own inferred type against the table's declared one.

    use super::*;

    fn literal_for(ty: &Type) -> String {
        match ty {
            Type::Num => "0".to_string(),
            Type::Text => "\"a\"".to_string(),
            Type::Bool => "true".to_string(),
            Type::Unit => "$".to_string(),
            Type::Function {
                parameters,
                return_type,
            } => {
                let names: Vec<String> = (0..parameters.len()).map(|i| format!("p{i}")).collect();
                format!("({}) => {}", names.join(", "), literal_for(return_type))
            }
            other => panic!("no literal synthesizer in this guard test for {other:?}"),
        }
    }

    /// `replace`/`replaceAll`/`repeat` reject some otherwise well-typed literals at
    /// compile time (an empty `from`, a non-positive `repeat` count, …) — these three
    /// need arguments chosen to satisfy that, not just their parameter types.
    fn arguments_for(name: &str, parameters: &[Type]) -> Vec<String> {
        match name {
            "replace" => vec!["\"a\"".to_string(), "\"z\"".to_string(), "1".to_string()],
            "replaceAll" => vec!["\"a\"".to_string(), "\"z\"".to_string()],
            "repeat" => vec!["2".to_string()],
            _ => parameters.iter().map(literal_for).collect(),
        }
    }

    /// `ty` with every `Type::Named` called `name` replaced by `with` — substitutes a
    /// table's generic placeholder (`map`'s `R`, `reduce`'s `A`) with a concrete type
    /// this test can actually check a call against.
    fn substitute(ty: &Type, name: &str, with: &Type) -> Type {
        match ty {
            Type::Named { name: n, .. } if n == name => with.clone(),
            Type::Array(elem) => Type::Array(Box::new(substitute(elem, name, with))),
            Type::Function {
                parameters,
                return_type,
            } => Type::Function {
                parameters: parameters
                    .iter()
                    .map(|p| substitute(p, name, with))
                    .collect(),
                return_type: Box::new(substitute(return_type, name, with)),
            },
            other => other.clone(),
        }
    }

    fn assert_table_matches_checker(
        receiver: &str,
        prelude: &str,
        table: Vec<(&str, Vec<Type>, Type)>,
    ) {
        for (name, parameters, ret) in table {
            let concrete =
                |ty: &Type| substitute(&substitute(ty, "R", &Type::Num), "A", &Type::Num);
            let parameters: Vec<Type> = parameters.iter().map(concrete).collect();
            let ret = concrete(&ret);
            let call = format!(
                "{receiver}.{name}({})",
                arguments_for(name, &parameters).join(", ")
            );
            let text = format!("^ = () -> Num => <\n  {prelude}\n  {call}\n  0\n>\n");
            let checked = check_text(Path::new("guard.qn"), &text)
                .unwrap_or_else(|error| panic!("`{call}` must check clean: {error:?}"));
            let start = text.find(&call).expect("the call is in its own text") as u32;
            let span = Span::in_root(start, start + call.len() as u32);
            let inferred = checked
                .types
                .get(&span)
                .unwrap_or_else(|| panic!("no recorded type for `{call}`"));
            assert_eq!(
                type_label(inferred),
                type_label(&ret),
                "`{name}`'s table return type disagrees with the checker"
            );
        }
    }

    #[test]
    fn text_method_table_matches_the_checker() {
        assert_table_matches_checker("s", "s = \"aaa\"", text_method_table());
    }

    #[test]
    fn array_method_table_matches_the_checker() {
        assert_table_matches_checker("xs", "xs = [0]", array_method_table(&Type::Num));
    }

    #[test]
    fn map_method_table_matches_the_checker() {
        assert_table_matches_checker(
            "m",
            "m = [|\"a\" => 0|]",
            map_method_table(&Type::Text, &Type::Num),
        );
    }

    #[test]
    fn set_method_table_matches_the_checker() {
        assert_table_matches_checker("xs", "xs = [|0|]", set_method_table(&Type::Num));
    }
}
