//! The fiber-sharing check: a `:=` value reachable from more than one fiber is a compile
//! error unless it is bound atomically (`@name := …`), reusing the deep-immutability
//! invariant `aliasing.rs` already proves (`=` freezes a value with no mutable path to it,
//! so sharing it costs nothing) rather than a second analysis. See
//! `docs/concurrency/README.md#sharing-state-across-fibers`.
//!
//! `net.@tcpServe(address, handler)` is the one place, until `http.@serve` lands, where
//! `handler` runs on a fiber of its own per call — [`FIBER_HANDLER_PRIMITIVES`] is the
//! whole surface a second primitive joins. Two things reaching `handler` are unsafe unless
//! atomic:
//!
//! - a non-atomic top-level `:=` binding, read or written by `handler` or by anything it
//!   calls transitively (walked the coarse way `ast::reachability` already does for the
//!   tree-shaker: every name an expression mentions, without resolving it — a mention of a
//!   global's name is that global even where a local happens to share it, which only ever
//!   widens what this check rejects, never narrows it);
//! - a `:=` local of the block `net.@tcpServe` is called from, captured by an inline
//!   lambda `handler`. A `:=` never shadows an existing binding of the same name (a
//!   reassignment resolves to whatever already has that name, however far outside the
//!   reassigning scope it lives — see `docs/mutation.md`), so a name the lambda touches
//!   that also names an outer local is that local, not a coincidence.
//!
//! Runs once, after `check_program`'s own item-by-item pass, once every top-level
//! binding's mutability and atomicity is settled.

use super::*;
use crate::ast::walk::try_for_each_subexpression;
use crate::ast::{Statement, at_primitive_name};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

/// Every primitive whose LAST argument is a handler run on its own fiber per call — the
/// bare `@` name paired with how a program spells calling it, for the diagnostic. Adding
/// `http.@serve` (lowered onto the same accept loop, see `docs/corelib/net.md`) is one
/// more entry here.
const FIBER_HANDLER_PRIMITIVES: &[(&str, &str)] = &[("tcpServe", "net.@tcpServe")];

/// One place a name is read or (re)declared/written. A `:=` declaration's own name counts
/// as touched wherever it sits — the tree-shaker's mention walk has no reason to see it
/// (assigning to a local mentions nothing callable), but sharing state is exactly a
/// question of what is written, not only what is called.
enum Touch<'a> {
    Read(&'a str, &'a Span),
    Write(&'a str, &'a Span),
}

impl<'a> Touch<'a> {
    fn name(&self) -> &'a str {
        match self {
            Touch::Read(name, _) | Touch::Write(name, _) => name,
        }
    }

    fn span(&self) -> &'a Span {
        match self {
            Touch::Read(_, span) | Touch::Write(_, span) => span,
        }
    }
}

impl TypeChecker {
    /// Every `net.@tcpServe`-shaped call in `program`, checked against the sharing rule.
    pub(super) fn check_fiber_sharing(&self, program: &Program) -> Result<(), TypeError> {
        let defined = index_top_level_functions(program);
        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) => {
                    self.check_fiber_launches_in(&declaration.body, &defined)?;
                }
                Item::VariableDeclaration(declaration) => {
                    self.check_fiber_launches_in(&declaration.value, &defined)?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        self.check_fiber_launches_in(&method.body, &defined)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Find every fiber-launching call inside `body` and check each one. `body` is also
    /// the scope a captured local is searched in — it is always a whole top-level item's
    /// own body/value, the "enclosing block" the sharing rule's local case names.
    fn check_fiber_launches_in(
        &self,
        body: &Expression,
        defined: &HashMap<&str, Vec<&Expression>>,
    ) -> Result<(), TypeError> {
        let mut result = Ok(());
        let _: ControlFlow<()> = try_for_each_subexpression(body, &mut |expression| {
            if let Expression::Call {
                function,
                arguments,
                span,
                ..
            } = expression
                && let Expression::Identifier { name, .. } = function.as_ref()
                && let Some(primitive) = fiber_handler_primitive(name)
                && let Err(error) =
                    self.check_fiber_launch(primitive, arguments, span, body, defined)
            {
                result = Err(error);
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        });
        result
    }

    /// Check one `net.@tcpServe(..., handler)` call: `handler`, and everything it calls
    /// transitively, may not reach a non-atomic top-level `:=` binding; an inline lambda
    /// `handler` may not additionally capture a `:=` local of `enclosing_body`.
    fn check_fiber_launch(
        &self,
        primitive: &'static str,
        arguments: &[Expression],
        call_span: &Span,
        enclosing_body: &Expression,
        defined: &HashMap<&str, Vec<&Expression>>,
    ) -> Result<(), TypeError> {
        let Some(handler) = arguments.last() else {
            return Ok(());
        };

        let roots: Vec<&Expression> = match handler {
            Expression::Lambda { body, .. } => vec![body.as_ref()],
            Expression::Identifier { name, .. } => {
                defined.get(name.as_str()).cloned().unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if let Some((name, touch)) = self.first_shared_global(&roots, defined) {
            return Err(self.shared_across_fibers(name, touch, call_span, primitive));
        }

        // The local-capture case only exists for an inline lambda: a handler reached by
        // name is a top-level function, which has no enclosing block to capture from.
        if let Expression::Lambda { body, .. } = handler {
            let mut outer_locals = Vec::new();
            walk(enclosing_body, Some(handler), &mut outer_locals);
            let captured_names: HashSet<&str> = outer_locals
                .into_iter()
                .filter_map(|touch| match touch {
                    Touch::Write(name, _)
                        if !self.env.is_top_level(name) && !self.env.is_atomic(name) =>
                    {
                        Some(name)
                    }
                    _ => None,
                })
                .collect();
            let mut handler_touches = Vec::new();
            walk(body, None, &mut handler_touches);
            for touch in &handler_touches {
                if captured_names.contains(touch.name()) {
                    return Err(self.shared_across_fibers(
                        touch.name(),
                        touch.span(),
                        call_span,
                        primitive,
                    ));
                }
            }
        }

        Ok(())
    }

    /// The first non-atomic top-level `:=` binding `roots` reach, directly or through a
    /// call to a top-level function (transitively — the search follows every call it in
    /// turn makes, the same way `ast::reachability` follows the whole program to build the
    /// tree-shaker's roots), with the span of the touch. `None` when nothing shared turns
    /// up.
    fn first_shared_global<'a>(
        &self,
        roots: &[&'a Expression],
        defined: &HashMap<&'a str, Vec<&'a Expression>>,
    ) -> Option<(&'a str, &'a Span)> {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut pending: Vec<&Expression> = roots.to_vec();
        while let Some(body) = pending.pop() {
            let mut touches = Vec::new();
            walk(body, None, &mut touches);
            for touch in &touches {
                let name = touch.name();
                if self.env.is_top_level(name)
                    && self.env.is_mutable(name)
                    && !self.env.is_atomic(name)
                {
                    return Some((name, touch.span()));
                }
            }
            for touch in &touches {
                let name = touch.name();
                if visited.insert(name)
                    && let Some(bodies) = defined.get(name)
                {
                    pending.extend(bodies.iter().copied());
                }
            }
        }
        None
    }

    fn shared_across_fibers(
        &self,
        name: &str,
        touch: &Span,
        call_span: &Span,
        primitive: &'static str,
    ) -> TypeError {
        TypeError::SharedAcrossFibers {
            name: name.to_string(),
            primitive,
            touch: touch.clone(),
            span: call_span.clone(),
        }
    }
}

/// The bare name of the leaf `@` primitive `name` refers to, when it is one of
/// [`FIBER_HANDLER_PRIMITIVES`] — `None` for every other call, `@`-marked or not.
fn fiber_handler_primitive(name: &str) -> Option<&'static str> {
    let bare = at_primitive_name(name)?;
    FIBER_HANDLER_PRIMITIVES
        .iter()
        .find(|(primitive, _)| *primitive == bare)
        .map(|(_, display)| *display)
}

/// Every top-level function's bodies, by name — an overload set's members share a name and
/// so share an entry, exactly like `ast::reachability::reachable_functions`'s own index.
fn index_top_level_functions(program: &Program) -> HashMap<&str, Vec<&Expression>> {
    let mut defined: HashMap<&str, Vec<&Expression>> = HashMap::new();
    for item in &program.items {
        if let Item::FunctionDeclaration(declaration) = item {
            defined
                .entry(declaration.name.as_str())
                .or_default()
                .push(&declaration.body);
        }
    }
    defined
}

/// Every name `expression` reads or (re)declares, with where — the shape of
/// `ast::reachability`'s mention walk, plus a `:=` declaration's own write target, and
/// `skip`: an expression to treat as a leaf (used to search a lambda's ENCLOSING scope for
/// captured locals without also collecting the lambda's own).
fn walk<'a>(expression: &'a Expression, skip: Option<&Expression>, out: &mut Vec<Touch<'a>>) {
    if let Some(skip) = skip
        && std::ptr::eq(expression, skip)
    {
        return;
    }
    match expression {
        // `it` is a method's receiver, never a shared binding of its own.
        Expression::Identifier { name, .. } if name == crate::ast::RECEIVER => {}
        Expression::Identifier { name, span } => out.push(Touch::Read(name, span)),
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. } => {}
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let crate::ast::InterpolationPart::Hole(hole) = part {
                    walk(hole, skip, out);
                }
            }
        }
        Expression::BinaryOperator { left, right, .. } => {
            walk(left, skip, out);
            walk(right, skip, out);
        }
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. }
        | Expression::Spread { expression, .. }
        | Expression::Lambda {
            body: expression, ..
        } => walk(expression, skip, out),
        Expression::Range { start, end, .. } => {
            walk(start, skip, out);
            walk(end, skip, out);
        }
        Expression::FieldAssign { target, value, .. }
        | Expression::IndexAssign { target, value, .. } => {
            walk(target, skip, out);
            walk(value, skip, out);
        }
        Expression::Index {
            expression, index, ..
        } => {
            walk(expression, skip, out);
            walk(index, skip, out);
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            walk(function, skip, out);
            for argument in arguments {
                walk(argument, skip, out);
            }
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            walk(condition, skip, out);
            walk(then, skip, out);
            walk(else_, skip, out);
        }
        Expression::Match {
            expression, arms, ..
        } => {
            walk(expression, skip, out);
            for arm in arms {
                walk(&arm.body, skip, out);
            }
        }
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            for element in elements {
                walk(element, skip, out);
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                walk(key, skip, out);
                walk(value, skip, out);
            }
        }
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            for (_, value) in fields {
                walk(value, skip, out);
            }
        }
        Expression::Block { statements, .. } => {
            for statement in statements {
                match statement {
                    Statement::Expression(e) => walk(e, skip, out),
                    Statement::Item(Item::VariableDeclaration(declaration)) => {
                        walk(&declaration.value, skip, out);
                        if declaration.mutable {
                            out.push(Touch::Write(&declaration.name, &declaration.span));
                        }
                    }
                    Statement::Item(Item::FunctionDeclaration(declaration)) => {
                        walk(&declaration.body, skip, out)
                    }
                    Statement::Item(Item::TypeDeclaration(declaration)) => {
                        for method in declaration.type_definition.methods() {
                            walk(&method.body, skip, out);
                        }
                    }
                }
            }
        }
    }
}
