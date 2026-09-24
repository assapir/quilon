//! The fiber-sharing check: a `:=` value reachable from more than one fiber is a compile
//! error unless it is bound atomically (`@name := …`), reusing the deep-immutability
//! invariant `aliasing.rs` already proves (`=` freezes a value with no mutable path to it,
//! so sharing it costs nothing) rather than a second analysis. See
//! `docs/concurrency/README.md#sharing-state-across-fibers`.
//!
//! `net.@tcpServe(address, handler)` and `http.@serve(address, handler)` (lowered onto the
//! same accept loop) are where `handler` runs on a fiber of its own per call —
//! [`FIBER_HANDLER_PRIMITIVES`] is the whole surface a further primitive joins. Two things
//! reaching `handler` are unsafe unless atomic:
//!
//! - a non-atomic top-level `:=` binding, read or written by `handler` or by anything it
//!   calls transitively (walked the coarse way `ast::reachability` already does for the
//!   tree-shaker: every name an expression mentions, without resolving it — a mention of a
//!   global's name is that global even where a local happens to share it, which only ever
//!   widens what this check rejects, never narrows it);
//! - a `:=` local declared as a DIRECT statement of the block `net.@tcpServe` is called
//!   from, captured by `handler` — an inline lambda, or a local (non-top-level) named
//!   handler reached by name, since no-hoisting means its own declaration already sits
//!   above the call. A `:=` never shadows an existing binding of the same name (a
//!   reassignment resolves to whatever already has that name, however far outside the
//!   reassigning scope it lives — see `docs/mutation.md`), so a name `handler` touches
//!   that also names one of these locals is that local, not a coincidence — and, unlike
//!   the global case above, this half stays PRECISE rather than coarse: it does not reach
//!   into a sibling function's or lambda's own body, whose locals belong to it alone.
//!
//! Runs once, after `check_program`'s own item-by-item pass, once every top-level
//! binding's mutability and atomicity is settled.

use super::*;
use crate::ast::walk::try_for_each_subexpression;
use crate::ast::{Statement, at_primitive_name};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

/// Every primitive whose LAST argument is a handler run on its own fiber per call — the
/// bare `@` name paired with how a program spells calling it, for the diagnostic. A
/// further primitive on the same accept loop is one more entry here.
const FIBER_HANDLER_PRIMITIVES: &[(&str, &str)] =
    &[("tcpServe", "net.@tcpServe"), ("serve", "http.@serve")];

/// One place a name is read or (re)declared/written — the callers below (the global-
/// reachability walk and its result) never need to tell which, only where. A `:=`
/// declaration's own name counts as touched wherever it sits: the tree-shaker's mention
/// walk this is shaped after has no reason to see it (assigning to a local mentions
/// nothing callable), but sharing state is exactly a question of what is written too.
struct Touch<'a> {
    name: &'a str,
    span: &'a Span,
}

impl TypeChecker {
    /// Every `net.@tcpServe`-shaped call in `program`, checked against the sharing rule.
    pub(super) fn check_fiber_sharing(&self, program: &Program) -> Result<(), TypeError> {
        let defined = index_top_level_functions(program);
        for item in &program.items {
            match item {
                Item::FunctionDeclaration(declaration) => {
                    self.check_fiber_launches_in(&declaration.body, &declaration.body, &defined)?;
                }
                Item::VariableDeclaration(declaration) => {
                    self.check_fiber_launches_in(&declaration.value, &declaration.value, &defined)?;
                }
                Item::TypeDeclaration(declaration) => {
                    for method in declaration.type_definition.methods() {
                        self.check_fiber_launches_in(&method.body, &method.body, &defined)?;
                    }
                }
                // A trap arm's body runs on its own fiber per signal, exactly like a
                // `net.@tcpServe` handler's — everything it reaches, directly or through a
                // call, is unsafe unless atomic. A trap is top-level, so there is no
                // enclosing block to capture a `:=` local from (the handler-capture half of
                // the rule); only the global half applies.
                Item::TrapDeclaration(trap) => {
                    for arm in &trap.arms {
                        self.check_fiber_launches_in(&arm.body, &arm.body, &defined)?;
                        if let Some((name, touch)) =
                            self.first_shared_global(&[&arm.body], &defined)
                        {
                            return Err(shared_across_fibers(name, touch, arm.body.span(), "!>"));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Find every fiber-launching call inside `expression` and check each one against
    /// `enclosing_scope` — the "enclosing block" the sharing rule's local case names, and
    /// where a handler passed by name is searched for. Unlike the generic
    /// `ast::walk::try_for_each_subexpression`, this walk UPDATES `enclosing_scope` on the
    /// way down: entering a nested named function's or a lambda's own body makes THAT body
    /// the enclosing scope for anything inside it, so a call sitting inside a locally
    /// declared helper resolves a same-named local against that helper's own body, never
    /// against a same-named local some other, unrelated nested closure happens to declare.
    fn check_fiber_launches_in(
        &self,
        expression: &Expression,
        enclosing_scope: &Expression,
        defined: &HashMap<&str, Vec<&Expression>>,
    ) -> Result<(), TypeError> {
        if let Expression::Call {
            function,
            arguments,
            span,
            ..
        } = expression
            && let Expression::Identifier { name, .. } = function.as_ref()
            && let Some(primitive) = fiber_handler_primitive(name)
        {
            self.check_fiber_launch(primitive, arguments, span, enclosing_scope, defined)?;
        }
        match expression {
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. }
            | Expression::Identifier { .. } => {}
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let crate::ast::InterpolationPart::Hole(hole) = part {
                        self.check_fiber_launches_in(hole, enclosing_scope, defined)?;
                    }
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.check_fiber_launches_in(function, enclosing_scope, defined)?;
                for argument in arguments {
                    self.check_fiber_launches_in(argument, enclosing_scope, defined)?;
                }
            }
            Expression::BinaryOperator { left, right, .. } => {
                self.check_fiber_launches_in(left, enclosing_scope, defined)?;
                self.check_fiber_launches_in(right, enclosing_scope, defined)?;
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::FieldAccess { expression, .. }
            | Expression::Spread { expression, .. } => {
                self.check_fiber_launches_in(expression, enclosing_scope, defined)?;
            }
            // A lambda's own body is a fresh enclosing scope for anything nested in it.
            Expression::Lambda { body, .. } => {
                self.check_fiber_launches_in(body, body, defined)?;
            }
            Expression::Range { start, end, .. } => {
                self.check_fiber_launches_in(start, enclosing_scope, defined)?;
                self.check_fiber_launches_in(end, enclosing_scope, defined)?;
            }
            Expression::FieldAssign { target, value, .. }
            | Expression::IndexAssign { target, value, .. } => {
                self.check_fiber_launches_in(target, enclosing_scope, defined)?;
                self.check_fiber_launches_in(value, enclosing_scope, defined)?;
            }
            Expression::Index {
                expression, index, ..
            } => {
                self.check_fiber_launches_in(expression, enclosing_scope, defined)?;
                self.check_fiber_launches_in(index, enclosing_scope, defined)?;
            }
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                self.check_fiber_launches_in(condition, enclosing_scope, defined)?;
                self.check_fiber_launches_in(then, enclosing_scope, defined)?;
                self.check_fiber_launches_in(else_, enclosing_scope, defined)?;
            }
            Expression::Match {
                expression, arms, ..
            } => {
                self.check_fiber_launches_in(expression, enclosing_scope, defined)?;
                for arm in arms {
                    self.check_fiber_launches_in(&arm.body, enclosing_scope, defined)?;
                }
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for element in elements {
                    self.check_fiber_launches_in(element, enclosing_scope, defined)?;
                }
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    self.check_fiber_launches_in(key, enclosing_scope, defined)?;
                    self.check_fiber_launches_in(value, enclosing_scope, defined)?;
                }
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, value) in fields {
                    self.check_fiber_launches_in(value, enclosing_scope, defined)?;
                }
            }
            Expression::Block { statements, .. } => {
                for statement in statements {
                    match statement {
                        Statement::Expression(e) => {
                            self.check_fiber_launches_in(e, enclosing_scope, defined)?
                        }
                        Statement::Item(Item::VariableDeclaration(declaration)) => self
                            .check_fiber_launches_in(
                                &declaration.value,
                                enclosing_scope,
                                defined,
                            )?,
                        // A nested named function's own body is a fresh enclosing scope,
                        // exactly like a lambda's — a local IT declares is not visible to
                        // a call that sits outside its body, and vice versa.
                        Statement::Item(Item::FunctionDeclaration(declaration)) => self
                            .check_fiber_launches_in(
                                &declaration.body,
                                &declaration.body,
                                defined,
                            )?,
                        Statement::Item(Item::TypeDeclaration(declaration)) => {
                            for method in declaration.type_definition.methods() {
                                self.check_fiber_launches_in(&method.body, &method.body, defined)?;
                            }
                        }
                        // A trap is a top-level-only item; never a block statement.
                        Statement::Item(Item::TrapDeclaration(_)) => {
                            unreachable!("a trap is a top-level-only item")
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Check one `net.@tcpServe(..., handler)` call: `handler`, and everything it calls
    /// transitively, may not reach a non-atomic `:=` binding; a `handler` whose own body
    /// this can see (an inline lambda, or a local — non-top-level — named handler, since
    /// no-hoisting means its declaration already sits above this call in
    /// `enclosing_body`) may not additionally capture a `:=` local of `enclosing_body`.
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

        // The body that actually runs on the fiber: an inline lambda's own body, or a
        // local named handler's — `None` for a top-level function name, which has no
        // enclosing block to capture from, or anything else this cannot see through.
        let handler_body: Option<&Expression> = match handler {
            Expression::Lambda { body, .. } => Some(body.as_ref()),
            Expression::Identifier { name, .. } if !defined.contains_key(name.as_str()) => {
                find_local_handler_body(enclosing_body, name)
            }
            _ => None,
        };

        let roots: Vec<&Expression> =
            handler_body
                .map(|body| vec![body])
                .unwrap_or_else(|| match handler {
                    Expression::Identifier { name, .. } => {
                        defined.get(name.as_str()).cloned().unwrap_or_default()
                    }
                    _ => Vec::new(),
                });
        if let Some((name, touch)) = self.first_shared_global(&roots, defined) {
            return Err(shared_across_fibers(name, touch, call_span, primitive));
        }

        if let Some(body) = handler_body {
            // Every `:=` name declared as a DIRECT statement of `enclosing_body` — never
            // reaching into some OTHER sibling function's or lambda's own body, whose
            // locals are private to it, not this block's, however coincidentally a name
            // there matches one the handler declares for itself. A name among these that
            // `body` also touches did not spring up twice by coincidence: a `:=`
            // declaration always reassigns whatever already has that name, however far
            // outside it lives, rather than shadowing it (`docs/mutation.md`), so it is
            // the SAME binding. `atomic` is read off the AST directly (`declaration.
            // atomic`, the DECLARING occurrence's own marker) rather than `self.env`,
            // which by now holds only what is still top-level — every local scope closed
            // as its own body finished checking.
            let mut local_names: HashSet<&str> = HashSet::new();
            let mut atomic_names: HashSet<&str> = HashSet::new();
            for (name, atomic) in direct_mutable_locals(enclosing_body) {
                local_names.insert(name);
                if atomic {
                    atomic_names.insert(name);
                }
            }
            let captured_names: HashSet<&str> = local_names
                .into_iter()
                .filter(|name| !self.env.is_top_level(name) && !atomic_names.contains(name))
                .collect();

            let mut handler_touches = Vec::new();
            walk(body, &mut handler_touches);
            for touch in &handler_touches {
                if captured_names.contains(touch.name) {
                    return Err(shared_across_fibers(
                        touch.name, touch.span, call_span, primitive,
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
            walk(body, &mut touches);
            for touch in &touches {
                let name = touch.name;
                if self.env.is_top_level(name)
                    && self.env.is_mutable(name)
                    && !self.env.is_atomic(name)
                {
                    return Some((name, touch.span));
                }
            }
            for touch in &touches {
                let name = touch.name;
                if visited.insert(name)
                    && let Some(bodies) = defined.get(name)
                {
                    pending.extend(bodies.iter().copied());
                }
            }
        }
        None
    }
}

/// Build the diagnostic: `name` is shared, touched at `touch`, through the launching call
/// at `call_span` — a plain function (not a `TypeChecker` method, since it reads none of
/// the checker's own state).
fn shared_across_fibers(
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

/// The body of a local (non-top-level) named handler `name` resolves to, declared as a
/// DIRECT statement of `scope` — the precise lexical scope `check_fiber_launches_in`
/// tracks down to, so this never has to guess which of several same-named declarations in
/// unrelated nested closures is the real one. Either a nested function declaration
/// (`h = (c :: net.Connection) => < … >`, which parses as its own `FunctionDeclaration`
/// inside the block, exactly like a top-level one) or a local variable bound to a lambda
/// VALUE (`h = c => …`, which parses as a plain binding when the parser sees no reason to
/// treat it as a declaration). No-hoisting means such a declaration already sits above its
/// use, so its own position within `scope`'s statements does not matter here.
fn find_local_handler_body<'a>(scope: &'a Expression, name: &str) -> Option<&'a Expression> {
    let Expression::Block { statements, .. } = scope else {
        return None;
    };
    statements.iter().find_map(|statement| match statement {
        Statement::Item(Item::FunctionDeclaration(declaration)) if declaration.name == name => {
            Some(&declaration.body)
        }
        Statement::Item(Item::VariableDeclaration(declaration)) if declaration.name == name => {
            match &declaration.value {
                Expression::Lambda { body, .. } => Some(body.as_ref()),
                _ => None,
            }
        }
        _ => None,
    })
}

/// Every `:=` name declared as a DIRECT statement of `scope`, paired with whether that
/// declaring occurrence was atomic — never reaching into a nested function's or lambda's
/// own body, whose locals belong to it alone. Not reused for the global-reachability walk
/// below (`walk`, `first_shared_global`), which wants the opposite: everything the handler
/// itself touches, including through code it declares and calls internally.
fn direct_mutable_locals(scope: &Expression) -> Vec<(&str, bool)> {
    let Expression::Block { statements, .. } = scope else {
        return Vec::new();
    };
    statements
        .iter()
        .filter_map(|statement| match statement {
            Statement::Item(Item::VariableDeclaration(declaration)) if declaration.mutable => {
                Some((declaration.name.as_str(), declaration.atomic))
            }
            _ => None,
        })
        .collect()
}

/// Every name `expression` reads or (re)declares, with where. Delegates the traversal to
/// the shared `ast::walk::try_for_each_subexpression` — which already descends into
/// lambda bodies, nested function bodies, and type methods, and visits every
/// `Expression::Block` itself — adding only what that generic walk has no reason to see:
/// a `:=` declaration's own write target (`hits := 5` mentions no callable, so the
/// tree-shaker's own walk skips it, but sharing state is exactly a question of what is
/// written too).
fn walk<'a>(expression: &'a Expression, out: &mut Vec<Touch<'a>>) {
    let _: ControlFlow<()> = try_for_each_subexpression(expression, &mut |expression| {
        match expression {
            // `it` is a method's receiver, never a shared binding of its own.
            Expression::Identifier { name, .. } if name == crate::ast::RECEIVER => {}
            Expression::Identifier { name, span } => out.push(Touch { name, span }),
            Expression::Block { statements, .. } => {
                for statement in statements {
                    if let Statement::Item(Item::VariableDeclaration(declaration)) = statement
                        && declaration.mutable
                    {
                        out.push(Touch {
                            name: &declaration.name,
                            span: &declaration.span,
                        });
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
}
