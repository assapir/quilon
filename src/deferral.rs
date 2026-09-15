//! Deferral analysis — the compiler's view of Quilon's `@` leaf-IO-primitive tier, and the
//! deferred-value taint that makes force-on-use real.
//!
//! The pass produces one thing: the **taint**. That is which expressions may evaluate to a
//! *deferred* value (a promise, from a value-returning `@` primitive like `@readStdin`), plus
//! the **force-set** (`force_sites`) — the exact spans where the code generator must force
//! such a value, because a strict primitive is about to read its bytes or it would escape.
//!
//! It reads no types and adds none. So the type checker is untouched, and a deferred `Text`
//! keeps the ordinary type `Text` — the load-bearing guardrail of the model.
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
//!
//! The same walk also enforces one rule about `@name := …` atomic bindings (see
//! `docs/concurrency/README.md#sharing-state-across-fibers`): a reassignment's right side
//! may not force a deferred value, because forcing parks the fiber mid-statement, and the
//! statement resumes holding a value read before the park — stale if another fiber wrote
//! the binding while this one was parked. The force-set this pass already computes is
//! exactly the set of force points, so the check is "did evaluating the right side add to
//! `force_sites`", read off the same walk rather than a second one.

use crate::ast::{
    Expression, InterpolationPart, Item, MethodDeclaration, Program, Statement,
    VariableDeclaration, at_primitive_name,
};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// The bare name of the value-returning stdin read primitive, reached by an importer as
/// `io.@readStdin()`; the call evaluates to a deferred `Text`.
const READ_PRIMITIVE: &str = "readStdin";

/// The bare name of the request-exchange socket primitive, reached by an importer as
/// `net.@tcpRequest(addr, req)`; the call evaluates to a deferred `Result`
/// (`Ok(responseBytes)` / `NotOk(message)`), read once forced.
const TCP_REQUEST_PRIMITIVE: &str = "tcpRequest";

/// The argument count `@tcpRequest` takes (`address`, `requestBytes`).
const TCP_REQUEST_ARITY: usize = 2;

/// What the analysis hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
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

/// A reassignment to an atomic binding whose right side forces a deferred value — the
/// binding-side twin of the locked rule that an atomic type's setter body may not force.
#[derive(Debug)]
pub struct AtomicReassignmentForced {
    pub name: String,
    pub span: Span,
}

/// Analyze `program`: the deferred-value taint and force-set — the whole codegen-visible
/// surface of the analysis. `Err` names the first atomic-binding reassignment, in program
/// order, whose right side forces a deferred value.
pub fn analyze(program: &Program) -> Result<DeferInfo, AtomicReassignmentForced> {
    // Every top-level atomic declaration's name — the fallback `is_atomic` reaches for
    // once a name is no longer in a lexical `Scope` at all, which is every named
    // function's own body: unlike a lambda, it starts from a fresh scope (see
    // `analyze_item`), so a global reassigned from inside one is otherwise invisible to
    // this check. A block-local atomic declaration needs no such fallback — `Scope`
    // already threads it to every statement after it in the same block.
    let global_atomic_names: HashSet<String> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::VariableDeclaration(v) if v.atomic => Some(v.name.clone()),
            _ => None,
        })
        .collect();
    let mut taint = Taint {
        global_atomic_names,
        ..Taint::default()
    };
    for item in &program.items {
        taint.analyze_item(item);
    }

    match taint.violation {
        Some(violation) => Err(violation),
        None => Ok(DeferInfo {
            force_sites: taint.force_sites,
        }),
    }
}

/// The deferred-value taint accumulator.
#[derive(Default)]
struct Taint {
    force_sites: HashSet<Span>,
    /// Every top-level atomic binding's name — see [`analyze`].
    global_atomic_names: HashSet<String>,
    /// The first atomic-reassignment violation found, in program order; later ones are not
    /// worth collecting; the whole point is to name one concrete fix.
    violation: Option<AtomicReassignmentForced>,
}

impl Taint {
    fn analyze_item(&mut self, item: &Item) {
        match item {
            // A function/method body is a strict slot: forcing its result keeps a promise from
            // escaping across the call boundary.
            Item::FunctionDeclaration(f) => self.strict(&f.body, &Scope::new()),
            Item::VariableDeclaration(v) => {
                let scope = Scope::new();
                if self.analyze_declaration_value(v, &scope) {
                    self.force_sites.insert(v.value.span().clone());
                }
            }
            Item::TypeDeclaration(t) => {
                for method in t.type_definition.methods() {
                    self.analyze_method(method);
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

    /// Whether `name` is atomic as reached from `env`: a block-local atomic declaration
    /// shadows the global fallback exactly as it shadows a global's deferredness (a local,
    /// non-atomic declaration of the same name records `false` and wins); a name absent
    /// from `env` altogether falls back to whether it is a top-level atomic declaration.
    fn is_atomic(&self, name: &str, env: &Scope) -> bool {
        match env.atomic_names.get(name) {
            Some(atomic) => *atomic,
            None => self.global_atomic_names.contains(name),
        }
    }

    /// Record `v` as the atomic-reassignment violation — once, the first one found — when
    /// it reassigns (mutable, not itself the declaring `@` occurrence) a binding atomic in
    /// `env`, and evaluating its value grew `force_sites` past `force_sites_before`: the
    /// right side forced a deferred value.
    fn check_atomic_reassignment(
        &mut self,
        v: &VariableDeclaration,
        env: &Scope,
        force_sites_before: usize,
    ) {
        if self.violation.is_none()
            && v.mutable
            && !v.atomic
            && self.force_sites.len() > force_sites_before
            && self.is_atomic(&v.name, env)
        {
            self.violation = Some(AtomicReassignmentForced {
                name: v.name.clone(),
                span: v.span.clone(),
            });
        }
    }

    /// Analyze a `:=`/`=` declaration or reassignment's value in `env`, checking along the
    /// way for the atomic-reassignment violation, and return whether the value is
    /// delivered to the caller still deferred (a `=`/`:=` binding is itself a lazy carrier
    /// — see [`Self::visit_block`] — so the caller, not this method, decides whether to
    /// force it).
    fn analyze_declaration_value(&mut self, v: &VariableDeclaration, env: &Scope) -> bool {
        let force_sites_before = self.force_sites.len();
        let deferred = self.visit(&v.value, env);
        self.check_atomic_reassignment(v, env, force_sites_before);
        deferred
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
            Expression::UnaryOperator { expression, .. }
            | Expression::Spread { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAccess { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAssign { target, value, .. }
            | Expression::IndexAssign { target, value, .. } => {
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
                    let deferred = self.analyze_declaration_value(v, &local);
                    if v.atomic {
                        local.bind_atomic(v.name.clone(), true);
                    }
                    local.bind(v.name.clone(), deferred);
                }
                Statement::Item(Item::FunctionDeclaration(f)) => {
                    self.strict(&f.body, &Scope::new());
                }
                // A block-declared type's methods are analyzed exactly like a
                // top-level type's (see `analyze_item`): each method body is its own
                // strict slot, in a fresh scope.
                Statement::Item(Item::TypeDeclaration(t)) => {
                    for method in t.type_definition.methods() {
                        self.analyze_method(method);
                    }
                }
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

/// A lexical scope mapping in-scope names to whether they hold a deferred value, and which
/// ones are atomic bindings. Names absent from `deferred_names` (parameters, pattern
/// bindings from a forced scrutinee, globals) are ready; names absent from `atomic_names`
/// fall back to [`Taint::is_atomic`]'s program-wide check.
#[derive(Clone, Default)]
struct Scope {
    deferred_names: HashMap<String, bool>,
    atomic_names: HashMap<String, bool>,
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

    fn bind_atomic(&mut self, name: String, atomic: bool) {
        self.atomic_names.insert(name, atomic);
    }
}

/// Whether `function`/`arguments` is a call to the `@readStdin` primitive (`@readStdin()`, no
/// arguments) — qualified (`io.@readStdin`) or, as inside `core.io` itself, still bare.
fn is_read_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. }
        if at_primitive_name(name) == Some(READ_PRIMITIVE))
        && arguments.is_empty()
}

/// Whether `function`/`arguments` is a call to the `@tcpRequest` primitive
/// (`@tcpRequest(address, requestBytes)`, exactly two arguments) — qualified or bare, the
/// same as [`is_read_call`].
fn is_tcp_request_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. }
        if at_primitive_name(name) == Some(TCP_REQUEST_PRIMITIVE))
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
        analyze(&program).expect("expected no atomic-reassignment violation")
    }

    /// The atomic-reassignment violation `src` is expected to raise.
    fn atomic_violation(src: &str) -> AtomicReassignmentForced {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        analyze(&program).expect_err("expected an atomic-reassignment violation")
    }

    /// The number of force sites in the program — the size of the force-set.
    fn force_count(src: &str) -> usize {
        info(src).force_sites.len()
    }

    #[test]
    fn pure_program_has_no_force_sites() {
        let i = info("^ = () -> Num => < 1 + 2 * 3 >");
        assert_eq!(i.force_sites.len(), 0);
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
        // Exactly one force: the `x` read inside the comparison. The binding stays lazy.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn read_directly_in_a_strict_slot_forces_at_the_call() {
        // No binding: the `@readStdin()` value is consumed strictly (compared) right away.
        let src = "<< core.io\n^ = () -> Num => < @readStdin() == \"hi\" ? 0 : 1 >";
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
    fn a_bare_read_at_the_root_is_still_deferred() {
        // A root program's own names stay bare — qualify renames nothing there — mirroring
        // `corelib/io.qn`'s own body, which calls `@readStdin` bare. `at_primitive_name` must
        // recognize the bare spelling too, or a root-level (or corelib-internal) call would
        // silently stop being tracked as a deferred producer while codegen still launches it.
        let src = "^ = () -> Num => < @readStdin() == \"hi\" ? 0 : 1 >";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn bound_tcp_request_is_deferred_and_forced_at_a_strict_use() {
        // `r = @tcpRequest(...)` binds a deferred Result (lazy); the match forces it once — the
        // same shape as a bound `@readStdin`, proving the taint tracks both producers.
        let src = "<< core.net\n^ = () -> Num => <\n  r = @tcpRequest(\"a:1\", \"b\")\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
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

    #[test]
    fn stream_file_result_is_never_a_deferred_value() {
        // `@streamFile` runs on the calling fiber: it is not in `produces_deferred`, so binding
        // its result produces no force site of its own — the match on it needs no force,
        // because it was never lazy to begin with.
        let src = "<< core.io\n^ = () -> Num => <\n  r = @streamFile(\"f\", 10, chunk => true)\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn a_deferred_argument_passed_to_stream_file_is_forced_at_the_argument() {
        // `@streamFile` is an ordinary call as far as its own arguments go: a deferred `Text`
        // flowing into its `path` argument is forced there, the same as any other call's
        // strict argument slot.
        let src = "<< core.io\n^ = () -> Num => <\n  p = @readStdin()\n  @streamFile(p, 10, chunk => true)\n  0\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn atomic_reassignment_forcing_a_deferred_value_is_rejected() {
        // The maintainer's own example: a top-level atomic global, forced on the right side
        // of its reassignment from a separate function.
        let src =
            "<< core.io\n@hits := 0\nbump = () -> $ => < hits := hits + @readStdin().length >";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn atomic_reassignment_directly_forcing_a_primitive_call_is_rejected() {
        let src = "<< core.io\n@hits := 0\nbump = () -> Num => < hits := @readStdin().length >";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn a_top_level_atomic_reassignment_forcing_a_deferred_value_is_rejected() {
        // The same rule at the top level, not from inside a separate function.
        let src = "<< core.io\n@hits := 0\nhits := hits + @readStdin().length";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn atomic_reassignment_reading_an_already_forced_binding_is_accepted() {
        // The accepted rewrite: force into a plain binding first, then reassign — the
        // reassignment's own right side reads an already-ready value.
        let src = "<< core.io\n@hits := 0\nbump = () -> $ => <\n  extra = @readStdin().length\n  hits := hits + extra\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_plain_mutable_reassignment_may_force_a_deferred_value() {
        // The rule is atomic-binding-specific: an ordinary `:=` global forcing a deferred
        // value on its reassignment's right side is untouched.
        let src = "<< core.io\ncounter := 0\nbump = () -> $ => < counter := counter + @readStdin().length >";
        assert_eq!(force_count(src), 1);
    }
}
