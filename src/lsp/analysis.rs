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
use crate::lexer::{Lexer, ROOT_FILE, Span, TokenKind};
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

// --- Go-to-definition -------------------------------------------------------

/// The span of the declaration binding the identifier at byte `offset` in the root
/// document, resolved against the import-linked `program` — so a name an import supplies
/// resolves to its declaration in the imported module's own file. `None` when the offset
/// is not on a resolvable identifier.
pub fn definition_at(program: &Program, offset: u32) -> Option<Span> {
    let mut finder = DefinitionFinder {
        offset,
        top_level: HashMap::new(),
        scopes: Vec::new(),
    };
    // All top-level names up front: the linked program lists imported items and the
    // document's own, and any one definition of a name answers a lookup (for an overload
    // set, the first member stands for the set).
    for item in &program.items {
        finder
            .top_level
            .entry(item.name().to_string())
            .or_insert_with(|| item.span().clone());
    }
    for item in &program.items {
        if let Some(found) = finder.item(item) {
            return Some(found);
        }
    }
    for block in &program.test_blocks {
        if let Some(found) = finder.expression(block) {
            return Some(found);
        }
    }
    None
}

/// The walk behind [`definition_at`]: descends every expression tracking lexical
/// bindings (parameters, block-local declarations, pattern bindings), and answers as soon
/// as it reaches the identifier whose span covers the offset.
struct DefinitionFinder {
    offset: u32,
    /// Every top-level name (the document's and its imports'), by declaration span.
    top_level: HashMap<String, Span>,
    /// The lexical scopes currently open, innermost last.
    scopes: Vec<HashMap<String, Span>>,
}

impl DefinitionFinder {
    fn covers(&self, span: &Span) -> bool {
        span.file == ROOT_FILE && span.start <= self.offset && self.offset < span.end
    }

    fn lookup(&self, name: &str) -> Option<Span> {
        for scope in self.scopes.iter().rev() {
            if let Some(span) = scope.get(name) {
                return Some(span.clone());
            }
        }
        self.top_level.get(name).cloned()
    }

    fn bind(&mut self, name: &str, span: &Span) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), span.clone());
        }
    }

    fn item(&mut self, item: &Item) -> Option<Span> {
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
                    let found = self.in_function(&method.parameters, &method.body);
                    self.scopes.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                None
            }
        }
    }

    /// Walk a body with `parameters` in scope, each binding its own name at its own span.
    fn in_function(&mut self, parameters: &[Parameter], body: &Expression) -> Option<Span> {
        let scope = parameters
            .iter()
            .map(|parameter| (parameter.name.clone(), parameter.span.clone()))
            .collect();
        self.scopes.push(scope);
        let found = self.expression(body);
        self.scopes.pop();
        found
    }

    fn statement(&mut self, statement: &Statement) -> Option<Span> {
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

    fn expression(&mut self, expression: &Expression) -> Option<Span> {
        match expression {
            Expression::Identifier { name, span } => match self.covers(span) {
                true => self.lookup(name),
                false => None,
            },
            Expression::Block { statements, .. } => {
                self.scopes.push(HashMap::new());
                let found = statements.iter().find_map(|s| self.statement(s));
                self.scopes.pop();
                found
            }
            Expression::Lambda {
                parameters, body, ..
            } => self.in_function(parameters, body),
            Expression::Call {
                function,
                arguments,
                ..
            } => self
                .expression(function)
                .or_else(|| arguments.iter().find_map(|a| self.expression(a))),
            Expression::Match {
                expression, arms, ..
            } => self.expression(expression).or_else(|| {
                arms.iter().find_map(|arm| {
                    self.scopes.push(HashMap::new());
                    self.pattern_bindings(&arm.pattern);
                    let found = self.expression(&arm.body);
                    self.scopes.pop();
                    found
                })
            }),
            Expression::BinaryOperator { left, right, .. } => self
                .expression(left)
                .or_else(|| self.expression(right)),
            Expression::UnaryOperator { expression, .. }
            | Expression::FieldAccess { expression, .. }
            | Expression::Spread { expression, .. } => self.expression(expression),
            Expression::FieldAssign { target, value, .. } => self
                .expression(target)
                .or_else(|| self.expression(value)),
            Expression::Index {
                expression, index, ..
            } => self
                .expression(expression)
                .or_else(|| self.expression(index)),
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => self
                .expression(condition)
                .or_else(|| self.expression(then))
                .or_else(|| self.expression(else_)),
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                elements.iter().find_map(|e| self.expression(e))
            }
            Expression::MapLiteral { entries, .. } => entries
                .iter()
                .find_map(|(k, v)| self.expression(k).or_else(|| self.expression(v))),
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                fields.iter().find_map(|(_, value)| self.expression(value))
            }
            Expression::Range { start, end, .. } => self
                .expression(start)
                .or_else(|| self.expression(end)),
            Expression::Interpolation { parts, .. } => parts.iter().find_map(|part| match part {
                InterpolationPart::Hole(hole) => self.expression(hole),
                InterpolationPart::Literal(_) => None,
            }),
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. } => None,
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
    /// The span of the whole `describe(...)` / `it(...)` call.
    pub span: Span,
}

/// Every test suite and case in the document's own parse, in document order: each
/// top-level `describe` block and, inside the blocks, every nested `describe` and every
/// `it` call. Empty when the document does not parse or has no test blocks.
pub fn test_lenses(text: &str) -> Vec<TestLens> {
    let Some(program) = parse_text(text) else {
        return Vec::new();
    };
    let mut lenses = Vec::new();
    for block in &program.test_blocks {
        walk_expressions(block, &mut |node| {
            if let Some(lens) = harness_call_lens(node) {
                lenses.push(lens);
            }
        });
    }
    lenses.sort_by_key(|lens| lens.span.start);
    lenses
}

/// The lens for `node`, when it is a module-qualified call of the harness's `describe`
/// or `it` (`test.describe(...)`, `core.test.it(...)`) — the pre-link spelling, since the
/// lenses read the document's own parse.
fn harness_call_lens(node: &Expression) -> Option<TestLens> {
    let Expression::Call {
        function,
        arguments,
        member_call: false,
        span,
    } = node
    else {
        return None;
    };
    let Expression::Identifier { name, .. } = function.as_ref() else {
        return None;
    };
    if !name.contains('.') {
        return None;
    }
    let kind = match display_name(name) {
        "describe" => TestLensKind::Suite,
        "it" => TestLensKind::Case,
        _ => return None,
    };
    let name = match arguments.first() {
        Some(Expression::String { value, .. }) => value.clone(),
        _ => String::new(),
    };
    Some(TestLens {
        kind,
        name,
        span: span.clone(),
    })
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
