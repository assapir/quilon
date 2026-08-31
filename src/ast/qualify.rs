//! Qualified-name resolution: the link-time pass behind `<< module` namespaces.
//!
//! An import binds its module's LAST path segment (`<< core.http` binds `http`), and the
//! module's exports are reached through that binding: `http.send(...)`, `http.Request { … }`,
//! `| http.Get =>`. The full path (`core.http.send`) always works too, and is the escape
//! hatch when two imported modules share a last segment.
//!
//! The pass works by renaming. Every top-level item of an imported module — private items
//! included, so an exported function's private helpers travel with it — is renamed to its
//! fully-qualified name (`send` in `core.http` becomes `core.http.send`), and every
//! reference is rewritten to match: the module's own bodies point at the renamed items,
//! and the importer's `http.send` resolves to `core.http.send` against what the module
//! exports. A `.` cannot appear in a written identifier, so the qualified names collide
//! with nothing, and the passes downstream (checker, codegen, reachability) see one flat
//! namespace of unique names and need no notion of modules at all.
//!
//! What is NOT renamed:
//! - `@` leaf IO primitives (`@sleep`): the `@` name stays bare and global once its
//!   module is imported — the sigil already marks its provenance.
//! - method and field names: they resolve against a receiver, never the top level.
//! - the built-ins that belong to no module (`Result`/`Ok`/`NotOk`, `assert`, matchers).

use crate::ast::nodes::{
    Expression, InterpolationPart, Item, MethodDeclaration, Parameter, Pattern, Program,
    Statement, Type, TypeDefinition, RECEIVER,
};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// A resolution failure, located at the reference or declaration that caused it.
#[derive(Debug)]
pub struct QualifyError {
    pub span: Span,
    pub message: String,
}

fn err<T>(span: &Span, message: String) -> Result<T, QualifyError> {
    Err(QualifyError {
        span: span.clone(),
        message,
    })
}

/// What one file's `<<` lines bind: the short names, the modules they mean, and each
/// module's exported surface. Built per file (the root program and every module resolve
/// only what THEY imported), consulted by [`resolve_program`] / [`qualify_module`].
#[derive(Default)]
pub struct ModuleScope {
    /// Short binding (`http`, `math`) -> canonical module name(s). More than one entry
    /// means the short name is ambiguous and only the full path resolves.
    aliases: HashMap<String, Vec<String>>,
    /// Canonical module name (`core.http`, or a file module's stem) -> the bare names it
    /// exports (functions, constants, type names, and the variants of exported sums).
    exports: HashMap<String, HashSet<String>>,
    /// Every name an import claims in this file — the short bindings plus the leading
    /// segments of dotted paths (`core`) — mapped to the import spelling that claims it,
    /// for the diagnostic. A declaration of one of these names is an error.
    claimed: HashMap<String, String>,
}

impl ModuleScope {
    /// Record one `<<` line: `alias` is the short binding (last segment / file stem),
    /// `canonical` the module's full name, `exports` its exported bare names.
    ///
    /// A second import binding the same short name is fine while both sides keep a longer
    /// full path to fall back on; a FILE module's canonical name IS its short name, so a
    /// collision involving one has no escape and is rejected here, at the import.
    pub fn add_import(
        &mut self,
        alias: &str,
        canonical: &str,
        exports: HashSet<String>,
        span: &Span,
    ) -> Result<(), QualifyError> {
        if let Some(existing) = self.aliases.get_mut(alias) {
            if !existing.iter().any(|c| c == canonical) {
                if canonical == alias || existing.iter().any(|c| c == alias) {
                    return err(
                        span,
                        format!(
                            "two imported modules are both named `{alias}`, and a module \
                             imported by file path has no longer name to fall back on; \
                             rename the file or drop one of the imports"
                        ),
                    );
                }
                existing.push(canonical.to_string());
            }
        } else {
            self.aliases
                .insert(alias.to_string(), vec![canonical.to_string()]);
        }
        self.exports
            .entry(canonical.to_string())
            .or_insert(exports);
        self.claimed
            .entry(alias.to_string())
            .or_insert_with(|| canonical.to_string());
        if let Some(first) = canonical.split('.').next()
            && first != canonical
        {
            self.claimed
                .entry(first.to_string())
                .or_insert_with(|| canonical.to_string());
        }
        Ok(())
    }

    /// Resolve a dotted reference (`http.send`, `core.http.send`) to its canonical name
    /// (`core.http.send`), requiring the member to be exported. Private and nonexistent
    /// members get the same answer, so an importer learns nothing about a module's insides.
    fn resolve_dotted(&self, name: &str, span: &Span) -> Result<String, QualifyError> {
        let (prefix, member) = name
            .rsplit_once('.')
            .expect("resolve_dotted is only called on dotted names");
        let canonical = if let Some(canonicals) = self.aliases.get(prefix) {
            match canonicals.as_slice() {
                [only] => only.as_str(),
                many => {
                    let options: Vec<String> = many
                        .iter()
                        .map(|c| format!("`{c}.{member}`"))
                        .collect();
                    return err(
                        span,
                        format!(
                            "`{prefix}` is ambiguous — more than one imported module binds \
                             it; write the full path: {}",
                            options.join(" or ")
                        ),
                    );
                }
            }
        } else if self.exports.contains_key(prefix) {
            prefix
        } else {
            return err(span, format!("`{prefix}` is not an imported module"));
        };
        let exported = self
            .exports
            .get(canonical)
            .is_some_and(|names| names.contains(member));
        if !exported {
            return err(
                span,
                format!("`{member}` is not exported by `{canonical}`"),
            );
        }
        Ok(format!("{canonical}.{member}"))
    }

    /// The import spelling that claims `name` in this file, if any — the reason a
    /// declaration of that name is rejected.
    fn claimed_by(&self, name: &str) -> Option<&str> {
        self.claimed.get(name).map(String::as_str)
    }
}

/// Rename every top-level item of an imported module to its fully-qualified name —
/// privates included — and rewrite the module's own bodies to match: bare references to
/// its own top level take the new names, and its dotted references resolve through ITS
/// import scope. Sum variants rename with their type (`Get` in `core.http` becomes
/// `core.http.Get`); `@` primitives keep their bare global names.
pub fn qualify_module(
    program: &mut Program,
    fqdn: &str,
    scope: &ModuleScope,
) -> Result<(), QualifyError> {
    let mut renames: HashMap<String, String> = HashMap::new();
    for item in &program.items {
        let name = item.name();
        if name.starts_with('@') {
            continue;
        }
        renames.insert(name.to_string(), format!("{fqdn}.{name}"));
        if let Item::TypeDeclaration(declaration) = item
            && let TypeDefinition::Sum { variants, .. } = &declaration.type_definition
        {
            for variant in variants {
                renames.insert(variant.name.clone(), format!("{fqdn}.{}", variant.name));
            }
        }
    }

    for item in &mut program.items {
        let (name, span) = match item {
            Item::VariableDeclaration(d) => (&mut d.name, &d.span),
            Item::FunctionDeclaration(d) => (&mut d.name, &d.span),
            Item::TypeDeclaration(d) => (&mut d.name, &d.span),
        };
        if let Some(spelling) = scope.claimed_by(name) {
            return err(
                span,
                claim_message(name, spelling),
            );
        }
        if let Some(renamed) = renames.get(name.as_str()) {
            *name = renamed.clone();
        }
        if let Item::TypeDeclaration(declaration) = item
            && let TypeDefinition::Sum { variants, .. } = &mut declaration.type_definition
        {
            for variant in variants {
                if let Some(renamed) = renames.get(variant.name.as_str()) {
                    variant.name = renamed.clone();
                }
            }
        }
    }

    let mut walker = Walker {
        renames,
        scope,
        locals: Vec::new(),
    };
    for item in &mut program.items {
        walker.item(item)?;
    }
    Ok(())
}

/// Resolve the ROOT program's qualified references against its imports, and enforce that
/// no declaration reuses a name an import claims. The root's own items keep their bare
/// names — only references change. Walks the test blocks too: a hoisted
/// `test.describe(...)` resolves the same way any other call does.
pub fn resolve_program(program: &mut Program, scope: &ModuleScope) -> Result<(), QualifyError> {
    let mut walker = Walker {
        renames: HashMap::new(),
        scope,
        locals: Vec::new(),
    };
    for item in &mut program.items {
        let (name, span) = match item {
            Item::VariableDeclaration(d) => (&d.name, &d.span),
            Item::FunctionDeclaration(d) => (&d.name, &d.span),
            Item::TypeDeclaration(d) => (&d.name, &d.span),
        };
        if let Some(spelling) = scope.claimed_by(name) {
            return err(span, claim_message(name, spelling));
        }
        walker.item(item)?;
    }
    for block in &mut program.test_blocks {
        walker.expression(block)?;
    }
    Ok(())
}

fn claim_message(name: &str, spelling: &str) -> String {
    format!(
        "`{name}` is claimed by the import of `{spelling}` — a binding may not reuse an \
         imported module's name"
    )
}

/// The scope-aware rewrite. `renames` maps a module's own bare top-level names to their
/// qualified spellings (empty for the root program); `locals` is the stack of names bound
/// by parameters, block bindings, and patterns, which shadow everything.
///
/// Every match in here is exhaustive with no catch-all, so a new AST variant fails to
/// compile until this pass says what happens to it — a silently unwalked variant would
/// mis-resolve whole programs.
struct Walker<'a> {
    renames: HashMap<String, String>,
    scope: &'a ModuleScope,
    locals: Vec<HashSet<String>>,
}

impl Walker<'_> {
    fn bound_locally(&self, name: &str) -> bool {
        self.locals.iter().any(|frame| frame.contains(name))
    }

    /// Record a binding (parameter, block local, pattern name) — after rejecting a name an
    /// import claims.
    fn declare(&mut self, name: &str, span: &Span) -> Result<(), QualifyError> {
        if let Some(spelling) = self.scope.claimed_by(name) {
            return err(span, claim_message(name, spelling));
        }
        if let Some(frame) = self.locals.last_mut() {
            frame.insert(name.to_string());
        }
        Ok(())
    }

    /// Rewrite one value reference: a dotted name resolves through the imports, a bare
    /// name takes the module's own rename if it has one (unless a local shadows it), and
    /// a bare name that IS an import binding is not a value at all.
    fn reference(&mut self, name: &mut String, span: &Span) -> Result<(), QualifyError> {
        if name.contains('.') {
            *name = self.scope.resolve_dotted(name, span)?;
            return Ok(());
        }
        if self.bound_locally(name) {
            return Ok(());
        }
        if let Some(renamed) = self.renames.get(name.as_str()) {
            *name = renamed.clone();
            return Ok(());
        }
        if let Some(spelling) = self.scope.claimed_by(name) {
            return err(
                span,
                format!(
                    "`{name}` names the imported module `{spelling}`, not a value — \
                     reach its exports as `{name}.<name>`"
                ),
            );
        }
        Ok(())
    }

    /// Rewrite a type-position name (`Type::Named`, a constructor's type, a pattern's
    /// constructor): dotted resolves, bare takes the module's own rename. Type names are
    /// never shadowed by value locals.
    fn type_reference(&mut self, name: &mut String, span: &Span) -> Result<(), QualifyError> {
        if name.contains('.') {
            *name = self.scope.resolve_dotted(name, span)?;
        } else if let Some(renamed) = self.renames.get(name.as_str()) {
            *name = renamed.clone();
        }
        Ok(())
    }

    fn item(&mut self, item: &mut Item) -> Result<(), QualifyError> {
        match item {
            Item::VariableDeclaration(declaration) => {
                if let Some(annotation) = &mut declaration.type_annotation {
                    self.type_(annotation, &declaration.span)?;
                }
                self.expression(&mut declaration.value)
            }
            Item::FunctionDeclaration(declaration) => {
                self.locals.push(HashSet::new());
                for parameter in &mut declaration.parameters {
                    self.parameter(parameter)?;
                }
                if let Some(annotation) = &mut declaration.return_type {
                    self.type_(annotation, &declaration.span)?;
                }
                if let Some(annotation) = &mut declaration.binding_type {
                    self.type_(annotation, &declaration.span)?;
                }
                let result = self.expression(&mut declaration.body);
                self.locals.pop();
                result
            }
            Item::TypeDeclaration(declaration) => {
                let span = declaration.span.clone();
                match &mut declaration.type_definition {
                    TypeDefinition::Sum { variants, methods } => {
                        for variant in variants {
                            for field in &mut variant.fields {
                                self.type_(field, &span)?;
                            }
                        }
                        for method in methods {
                            self.method(method)?;
                        }
                    }
                    TypeDefinition::Record { fields, methods } => {
                        for (_, field_type) in fields {
                            self.type_(field_type, &span)?;
                        }
                        for method in methods {
                            self.method(method)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn method(&mut self, method: &mut MethodDeclaration) -> Result<(), QualifyError> {
        self.locals.push(HashSet::new());
        if let Some(frame) = self.locals.last_mut() {
            frame.insert(RECEIVER.to_string());
        }
        for parameter in &mut method.parameters {
            self.parameter(parameter)?;
        }
        if let Some(annotation) = &mut method.return_type {
            self.type_(annotation, &method.span)?;
        }
        let result = self.expression(&mut method.body);
        self.locals.pop();
        result
    }

    fn parameter(&mut self, parameter: &mut Parameter) -> Result<(), QualifyError> {
        if let Some(annotation) = &mut parameter.type_annotation {
            self.type_(annotation, &parameter.span)?;
        }
        self.declare(&parameter.name, &parameter.span)
    }

    fn statement(&mut self, statement: &mut Statement) -> Result<(), QualifyError> {
        match statement {
            Statement::Expression(expression) => self.expression(expression),
            // A block-local binding: its value is resolved BEFORE the name binds (no
            // hoisting — `x = x + 1` reads the outer `x`)…
            Statement::Item(Item::VariableDeclaration(declaration)) => {
                if let Some(annotation) = &mut declaration.type_annotation {
                    self.type_(annotation, &declaration.span)?;
                }
                self.expression(&mut declaration.value)?;
                self.declare(&declaration.name, &declaration.span)
            }
            // …while a local function is in scope for its own body (self-recursion).
            Statement::Item(Item::FunctionDeclaration(declaration)) => {
                self.declare(&declaration.name, &declaration.span)?;
                self.locals.push(HashSet::new());
                for parameter in &mut declaration.parameters {
                    self.parameter(parameter)?;
                }
                if let Some(annotation) = &mut declaration.return_type {
                    self.type_(annotation, &declaration.span)?;
                }
                if let Some(annotation) = &mut declaration.binding_type {
                    self.type_(annotation, &declaration.span)?;
                }
                let result = self.expression(&mut declaration.body);
                self.locals.pop();
                result
            }
            Statement::Item(item @ Item::TypeDeclaration(_)) => self.item(item),
        }
    }

    fn expression(&mut self, expression: &mut Expression) -> Result<(), QualifyError> {
        match expression {
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. } => Ok(()),
            Expression::Identifier { name, span } => self.reference(name, span),
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    match part {
                        InterpolationPart::Literal(_) => {}
                        InterpolationPart::Hole(hole) => self.expression(hole)?,
                    }
                }
                Ok(())
            }
            Expression::Call {
                function,
                arguments,
                member_call,
                ..
            } => {
                // A member call's name resolves against its receiver's type, never the
                // top level — so it is not a reference this pass may touch.
                if !*member_call {
                    self.expression(function)?;
                }
                for argument in arguments {
                    self.expression(argument)?;
                }
                Ok(())
            }
            Expression::BinaryOperator { left, right, .. } => {
                self.expression(left)?;
                self.expression(right)
            }
            Expression::UnaryOperator { expression, .. } => self.expression(expression),
            Expression::Lambda {
                parameters,
                return_type,
                body,
                span,
            } => {
                self.locals.push(HashSet::new());
                for parameter in parameters {
                    self.parameter(parameter)?;
                }
                if let Some(annotation) = return_type {
                    self.type_(annotation, span)?;
                }
                let result = self.expression(body);
                self.locals.pop();
                result
            }
            Expression::Pipeline { left, right, .. } => {
                self.expression(left)?;
                self.expression(right)
            }
            Expression::Block { statements, .. } => {
                self.locals.push(HashSet::new());
                let result = statements
                    .iter_mut()
                    .try_for_each(|statement| self.statement(statement));
                self.locals.pop();
                result
            }
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                self.expression(condition)?;
                self.expression(then)?;
                self.expression(else_)
            }
            Expression::Match {
                expression, arms, ..
            } => {
                self.expression(expression)?;
                for arm in arms {
                    self.locals.push(HashSet::new());
                    let result = self
                        .pattern(&mut arm.pattern)
                        .and_then(|()| self.expression(&mut arm.body));
                    self.locals.pop();
                    result?;
                }
                Ok(())
            }
            Expression::FieldAccess { expression, .. } => self.expression(expression),
            Expression::FieldAssign { target, value, .. } => {
                self.expression(target)?;
                self.expression(value)
            }
            Expression::Index {
                expression, index, ..
            } => {
                self.expression(expression)?;
                self.expression(index)
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                elements
                    .iter_mut()
                    .try_for_each(|element| self.expression(element))
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    self.expression(key)?;
                    self.expression(value)?;
                }
                Ok(())
            }
            Expression::Record { fields, .. } => fields
                .iter_mut()
                .try_for_each(|(_, value)| self.expression(value)),
            Expression::Constructor {
                type_name,
                fields,
                span,
            } => {
                self.type_reference(type_name, span)?;
                fields
                    .iter_mut()
                    .try_for_each(|(_, value)| self.expression(value))
            }
            Expression::Range { start, end, .. } => {
                self.expression(start)?;
                self.expression(end)
            }
            Expression::Spread { expression, .. } => self.expression(expression),
        }
    }

    fn pattern(&mut self, pattern: &mut Pattern) -> Result<(), QualifyError> {
        match pattern {
            Pattern::Identifier { name, span } => self.declare(name, span),
            Pattern::Number { .. } | Pattern::Wildcard { .. } => Ok(()),
            Pattern::Constructor {
                name,
                arguments,
                span,
            } => {
                self.type_reference(name, span)?;
                arguments
                    .iter_mut()
                    .try_for_each(|argument| self.pattern(argument))
            }
        }
    }

    fn type_(&mut self, ty: &mut Type, span: &Span) -> Result<(), QualifyError> {
        match ty {
            Type::Num | Type::Text | Type::Bool | Type::Unit => Ok(()),
            Type::Array(element) | Type::Set(element) => self.type_(element, span),
            Type::Map(key, value) => {
                self.type_(key, span)?;
                self.type_(value, span)
            }
            Type::Record(fields) => fields
                .iter_mut()
                .try_for_each(|(_, field_type)| self.type_(field_type, span)),
            Type::Named { name, .. } => self.type_reference(name, span),
            Type::Generic { arguments, .. } => arguments
                .iter_mut()
                .try_for_each(|argument| self.type_(argument, span)),
            Type::Function {
                parameters,
                return_type,
            } => {
                for parameter in parameters {
                    self.type_(parameter, span)?;
                }
                self.type_(return_type, span)
            }
            Type::Sum { variants, .. } => variants.iter_mut().try_for_each(|variant| {
                variant
                    .fields
                    .iter_mut()
                    .try_for_each(|field| self.type_(field, span))
            }),
        }
    }
}
