//! Deferral analysis — the compiler's view of Quilon's `@` leaf-IO-primitive tier, and the
//! deferred-value taint that makes force-on-use real.
//!
//! Two products, both read no types and add none — so the type checker is untouched and a
//! deferred `Text` keeps the ordinary type `Text` (the load-bearing guardrail of the model):
//!
//!  * `uses_deferral` — whether the program reaches ANY `@` primitive. Gates running the
//!    entry on a scheduler fiber; a program that uses none is compiled byte-identically.
//!  * the **taint** — which expressions may evaluate to a *deferred* value (a promise, from a
//!    value-returning `@` primitive like `@readStdin`), and the **force-set** (`force_sites`): the
//!    exact expression spans where the code generator must force such a value because a strict
//!    primitive is about to read its bytes, or it would otherwise escape.
//!
//! Taint is a forward dataflow. A value is deferred iff it flows from `@readStdin` through only
//! *lazy carriers* — a `=` binding, and the arms/result of `?`/ternary/blocks — without
//! crossing a *strict* slot. At every strict slot (arithmetic/comparison/logical operands,
//! `?`/ternary/match scrutinee, `print`/`eprint`/`write` and native/`@` args, indexing, field
//! and array/record construction, interpolation holes, and a function/method/lambda body
//! result) a deferred child is forced. Forcing at the body result and at call arguments keeps
//! a promise inside the one function body it was born in (this step launches independent IO
//! and overlaps it; cross-function promise pipelining — a function *returning* a deferred
//! value — is a later step). Only tainted spans get forces, so pure code pays nothing.

use crate::ast::walk::for_each_subexpression;
use crate::ast::{
    Expression, InterpolationPart, Item, MethodDeclaration, Program, Statement, TypeDefinition,
};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// The corelib name of the value-returning stdin read primitive; `@readStdin()` evaluates to
/// a deferred `Text`.
const READ_PRIMITIVE: &str = "@readStdin";

/// The internal name of the request-exchange socket primitive; `@tcpRequest(addr, req)`
/// evaluates to a deferred `Result` (`Ok(responseBytes)` / `NotOk(message)`), read once forced.
const TCP_REQUEST_PRIMITIVE: &str = "@tcpRequest";

/// The argument count `@tcpRequest` takes (`address`, `requestBytes`).
const TCP_REQUEST_ARITY: usize = 2;

/// What the analysis hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
    /// Whether any `@` leaf IO primitive is reachable. Gates running the entry on a
    /// scheduler fiber: a program that uses no `@` primitive is byte-identical to before.
    pub uses_deferral: bool,
    /// The force-set: spans of expressions whose generated value codegen must force in
    /// place, because a deferred value sits where a strict primitive reads its bytes or
    /// would escape. Empty for pure programs — the whole codegen-visible surface of the
    /// taint analysis.
    force_sites: HashSet<Span>,
}

impl DeferInfo {
    /// Whether the value produced for the expression at `span` must be forced in place.
    pub fn is_force_site(&self, span: &Span) -> bool {
        self.force_sites.contains(span)
    }
}

/// Analyze `program`: whether it reaches an `@` primitive, plus the deferred-value taint and
/// force-set — the whole codegen-visible surface of the analysis.
pub fn analyze(program: &Program) -> DeferInfo {
    let uses_deferral = program.items.iter().any(|item| match item {
        Item::FunctionDeclaration(f) => references_at_primitive(&f.body),
        Item::VariableDeclaration(v) => references_at_primitive(&v.value),
        Item::TypeDeclaration(_) => false,
    });

    let mut taint = Taint::default();
    for item in &program.items {
        taint.analyze_item(item);
    }

    DeferInfo {
        uses_deferral,
        force_sites: taint.force_sites,
    }
}

/// Whether any `@`-primitive reference appears anywhere in `expression`. An `@` name can only ever
/// name a leaf IO primitive (the parser reserves the `@`), so any `@`-prefixed identifier —
/// called directly, piped into, or otherwise — counts.
fn references_at_primitive(expression: &Expression) -> bool {
    let mut found = false;
    for_each_subexpression(expression, &mut |e| {
        if let Expression::Identifier { name, .. } = e
            && name.starts_with('@')
        {
            found = true;
        }
    });
    found
}

/// The deferred-value taint accumulator.
#[derive(Default)]
struct Taint {
    force_sites: HashSet<Span>,
}

impl Taint {
    fn analyze_item(&mut self, item: &Item) {
        match item {
            // A function/method body is a strict slot: forcing its result keeps a promise from
            // escaping across the call boundary.
            Item::FunctionDeclaration(f) => self.strict(&f.body, &Scope::new()),
            Item::VariableDeclaration(v) => self.strict(&v.value, &Scope::new()),
            Item::TypeDeclaration(t) => {
                if let TypeDefinition::Record { methods, .. } = &t.type_definition {
                    for method in methods {
                        self.analyze_method(method);
                    }
                }
            }
        }
    }

    fn analyze_method(&mut self, method: &MethodDeclaration) {
        self.strict(&method.body, &Scope::new());
    }

    /// Visit `expression` in a STRICT slot: analyze it, and if its value is deferred, force it here.
    fn strict(&mut self, expression: &Expression, env: &Scope) {
        if self.visit(expression, env) {
            self.force_sites.insert(expression.span().clone());
        }
    }

    /// Analyze `expression`, recording forces for its own strict children, and return whether its
    /// value is delivered to the parent still deferred (i.e. it reached here through lazy
    /// carriers only). The parent decides whether to force it, via [`Self::strict`].
    fn visit(&mut self, expression: &Expression, env: &Scope) -> bool {
        match expression {
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. } => false,
            Expression::Identifier { name, .. } => env.is_deferred(name),

            // A value-returning `@` primitive (`@readStdin`, `@tcpRequest`) is the only kind of
            // deferred-producing call; every other call delivers a ready value (its own body
            // forced its result). The callee expression and the arguments are all strict slots
            // (a deferred value used inside `function` — e.g. a called lambda — or passed as an
            // argument is forced there).
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.strict(function, env);
                for arg in arguments {
                    self.strict(arg, env);
                }
                produces_deferred(function, arguments)
            }

            Expression::BinaryOperator { left, right, .. } => {
                self.strict(left, env);
                self.strict(right, env);
                false
            }
            Expression::Pipeline { left, right, .. } => {
                self.strict(left, env);
                self.strict(right, env);
                false
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::Spread { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAccess { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAssign { target, value, .. } => {
                self.strict(target, env);
                self.strict(value, env);
                false
            }
            Expression::Index {
                expression, index, ..
            } => {
                self.strict(expression, env);
                self.strict(index, env);
                false
            }
            Expression::Range { start, end, .. } => {
                self.strict(start, env);
                self.strict(end, env);
                false
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for element in elements {
                    self.strict(element, env);
                }
                false
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    self.strict(key, env);
                    self.strict(value, env);
                }
                false
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, value) in fields {
                    self.strict(value, env);
                }
                false
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let InterpolationPart::Hole(hole) = part {
                        self.strict(hole, env);
                    }
                }
                false
            }
            // A lambda body result is strict (a closure never returns a promise). The outer
            // scope is visible so a captured deferred value used strictly inside is forced.
            Expression::Lambda { body, .. } => {
                self.strict(body, env);
                false
            }

            // Lazy carriers: the arms/result flow the value through without forcing, so the
            // If/Match/Block delivers deferred iff any branch does — the parent slot forces it.
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                self.strict(condition, env);
                let then_deferred = self.visit(then, env);
                let else_deferred = self.visit(else_, env);
                then_deferred || else_deferred
            }
            Expression::Match {
                expression, arms, ..
            } => {
                self.strict(expression, env);
                let mut any = false;
                for arm in arms {
                    any |= self.visit(&arm.body, env);
                }
                any
            }
            Expression::Block { statements, .. } => self.visit_block(statements, env),
        }
    }

    /// A block introduces a scope. Bindings carry their value's deferredness (a `=` is lazy);
    /// non-final statement values are discarded (not forced — the launch still runs); the
    /// final expression's value is the block's value, delivered to the block's own slot.
    fn visit_block(&mut self, statements: &[Statement], env: &Scope) -> bool {
        let mut local = env.child();
        let last = statements.len().saturating_sub(1);
        let mut result_deferred = false;
        for (index, statement) in statements.iter().enumerate() {
            match statement {
                Statement::Item(Item::VariableDeclaration(v)) => {
                    let deferred = self.visit(&v.value, &local);
                    local.bind(v.name.clone(), deferred);
                }
                Statement::Item(Item::FunctionDeclaration(f)) => {
                    self.strict(&f.body, &Scope::new());
                }
                Statement::Item(Item::TypeDeclaration(_)) => {}
                Statement::Expression(e) => {
                    if index == last {
                        result_deferred = self.visit(e, &local);
                    } else {
                        // Discarded: analyze for its own strict children, but the value itself
                        // need not be forced — its launch already ran (eager launch).
                        let _ = self.visit(e, &local);
                    }
                }
            }
        }
        result_deferred
    }
}

/// A lexical scope mapping in-scope names to whether they hold a deferred value. Names absent
/// from the map (parameters, pattern bindings from a forced scrutinee, globals) are ready.
#[derive(Clone, Default)]
struct Scope {
    deferred_names: HashMap<String, bool>,
}

impl Scope {
    fn new() -> Self {
        Scope::default()
    }

    fn child(&self) -> Self {
        self.clone()
    }

    fn bind(&mut self, name: String, deferred: bool) {
        self.deferred_names.insert(name, deferred);
    }

    fn is_deferred(&self, name: &str) -> bool {
        self.deferred_names.get(name).copied().unwrap_or(false)
    }
}

/// Whether `function`/`arguments` is a call to the `@readStdin` primitive (`@readStdin()`, no arguments).
fn is_read_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. } if name == READ_PRIMITIVE)
        && arguments.is_empty()
}

/// Whether `function`/`arguments` is a call to the `@tcpRequest` primitive
/// (`@tcpRequest(address, requestBytes)`, exactly two arguments).
fn is_tcp_request_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. } if name == TCP_REQUEST_PRIMITIVE)
        && arguments.len() == TCP_REQUEST_ARITY
}

/// Whether `function`/`arguments` is a call to a value-returning `@` primitive — one that hands
/// back a DEFERRED value the taint must track: `@readStdin()` (a deferred `Text` line) or
/// `@tcpRequest(addr, req)` (a deferred `Result`). Matched on name AND arity, so a call that does
/// not fit the primitive's signature is not treated as deferred. Effect-only primitives like
/// `@sleep` (which yields `$`) are never deferred and so never appear here.
fn produces_deferred(function: &Expression, arguments: &[Expression]) -> bool {
    is_read_call(function, arguments) || is_tcp_request_call(function, arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser;

    fn info(src: &str) -> DeferInfo {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        analyze(&program)
    }

    fn uses_deferral(src: &str) -> bool {
        info(src).uses_deferral
    }

    /// The number of force sites in the program — the size of the force-set.
    fn force_count(src: &str) -> usize {
        info(src).force_sites.len()
    }

    #[test]
    fn pure_program_uses_no_deferral() {
        let i = info("^ = () -> Num => 1 + 2 * 3");
        assert!(!i.uses_deferral);
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn a_sleep_call_marks_deferral() {
        assert!(uses_deferral("^ = () -> $ => <\n  @sleep(1)\n  $\n>"));
    }

    #[test]
    fn sleep_reached_through_a_helper_marks_deferral() {
        assert!(uses_deferral(
            "nap = () -> $ => @sleep(1)\n^ = () -> $ => nap()"
        ));
    }

    #[test]
    fn sleep_piped_in_still_marks_deferral() {
        assert!(uses_deferral("^ = () -> $ => 1 |> @sleep"));
    }

    #[test]
    fn effect_only_sleep_is_never_a_deferred_value() {
        // `@sleep` returns `$`, not a value: it is never deferred and never forced.
        let i = info("^ = () -> $ => <\n  @sleep(1)\n  $\n>");
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn bound_read_is_deferred_and_forced_at_a_strict_use() {
        // `x = @readStdin()` binds a deferred Text (lazy); the comparison forces it once.
        let src = "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  x == \"hi\" ? 0 : 1\n>";
        let i = info(src);
        assert!(i.uses_deferral);
        // Exactly one force: the `x` read inside the comparison. The binding stays lazy.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn read_directly_in_a_strict_slot_forces_at_the_call() {
        // No binding: the `@readStdin()` value is consumed strictly (compared) right away.
        let src = "<< core.io\n^ = () -> Num => @readStdin() == \"hi\" ? 0 : 1";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn read_passed_to_a_call_forces_at_the_argument() {
        let src = "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  print(x)\n  0\n>";
        // The `print(x)` argument is a strict slot: one force.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_bound_but_unused_read_is_not_forced() {
        // Launched (eager) but never read strictly: no force site. The launch still runs.
        let src = "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  0\n>";
        let i = info(src);
        assert!(i.uses_deferral);
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn read_flows_lazily_through_a_second_binding() {
        let src =
            "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  y = x\n  y == \"hi\" ? 0 : 1\n>";
        // Two lazy bindings, forced once at the comparison.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn tcp_request_marks_deferral() {
        assert!(uses_deferral(
            "<< core.net\n^ = () -> Num => <\n  @tcpRequest(\"a:1\", \"b\")\n  0\n>"
        ));
    }

    #[test]
    fn bound_tcp_request_is_deferred_and_forced_at_a_strict_use() {
        // `r = @tcpRequest(...)` binds a deferred Result (lazy); the match forces it once — the
        // same shape as a bound `@readStdin`, proving the taint tracks both producers.
        let src = "<< core.net\n^ = () -> Num => <\n  r = @tcpRequest(\"a:1\", \"b\")\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
        let i = info(src);
        assert!(i.uses_deferral);
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn tcp_request_with_wrong_arity_is_not_deferred() {
        // A `@tcpRequest` reference that does not fit the primitive's two-argument signature is
        // not treated as a deferred producer: no value flows out deferred, so nothing is forced.
        let src = "<< core.net\n^ = () -> Num => <\n  r = @tcpRequest(\"a:1\")\n  0\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn read_through_a_ternary_arm_forces_at_the_result_use() {
        // Ternary arms are lazy carriers: the deferred value survives the `?` and is forced
        // where the ternary's result is used strictly (the outer comparison).
        let src = "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  chosen = true ? x : \"z\"\n  chosen == \"hi\" ? 0 : 1\n>";
        assert_eq!(force_count(src), 1);
    }
}
