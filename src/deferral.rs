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

use crate::ast::{Expr, InterpPart, Item, MethodDecl, Program, Statement, TypeDef};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// The corelib name of the value-returning stdin read primitive; `@readStdin()` evaluates to
/// a deferred `Text`.
const READ_PRIMITIVE: &str = "@readStdin";

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
        Item::FunctionDecl(f) => references_at_primitive(&f.body),
        Item::VarDecl(v) => references_at_primitive(&v.value),
        Item::TypeDecl(_) => false,
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

/// Whether any `@`-primitive reference appears anywhere in `expr`. An `@` name can only ever
/// name a leaf IO primitive (the parser reserves the `@`), so any `@`-prefixed identifier —
/// called directly, piped into, or otherwise — counts.
fn references_at_primitive(expr: &Expr) -> bool {
    let mut found = false;
    for_each_subexpr(expr, &mut |e| {
        if let Expr::Ident { name, .. } = e
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
            Item::FunctionDecl(f) => self.strict(&f.body, &Scope::new()),
            Item::VarDecl(v) => self.strict(&v.value, &Scope::new()),
            Item::TypeDecl(t) => {
                if let TypeDef::Record { methods, .. } = &t.type_def {
                    for method in methods {
                        self.analyze_method(method);
                    }
                }
            }
        }
    }

    fn analyze_method(&mut self, method: &MethodDecl) {
        self.strict(&method.body, &Scope::new());
    }

    /// Visit `expr` in a STRICT slot: analyze it, and if its value is deferred, force it here.
    fn strict(&mut self, expr: &Expr, env: &Scope) {
        if self.visit(expr, env) {
            self.force_sites.insert(expr.span().clone());
        }
    }

    /// Analyze `expr`, recording forces for its own strict children, and return whether its
    /// value is delivered to the parent still deferred (i.e. it reached here through lazy
    /// carriers only). The parent decides whether to force it, via [`Self::strict`].
    fn visit(&mut self, expr: &Expr, env: &Scope) -> bool {
        match expr {
            Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Unit { .. } => {
                false
            }
            Expr::Ident { name, .. } => env.is_deferred(name),

            // `@readStdin()` is the one deferred-producing primitive; every other call delivers
            // a ready value (its own body forced its result). The callee expression and the
            // arguments are all strict slots (a deferred value used inside `func` — e.g. a
            // called lambda — or passed as an argument is forced there).
            Expr::Call { func, args, .. } => {
                self.strict(func, env);
                for arg in args {
                    self.strict(arg, env);
                }
                is_read_call(func, args)
            }

            Expr::BinOp { left, right, .. } => {
                self.strict(left, env);
                self.strict(right, env);
                false
            }
            Expr::Pipeline { left, right, .. } => {
                self.strict(left, env);
                self.strict(right, env);
                false
            }
            Expr::UnaryOp { expr, .. } | Expr::Spread { expr, .. } => {
                self.strict(expr, env);
                false
            }
            Expr::FieldAccess { expr, .. } => {
                self.strict(expr, env);
                false
            }
            Expr::FieldAssign { target, value, .. } => {
                self.strict(target, env);
                self.strict(value, env);
                false
            }
            Expr::Index { expr, index, .. } => {
                self.strict(expr, env);
                self.strict(index, env);
                false
            }
            Expr::Range { start, end, .. } => {
                self.strict(start, env);
                self.strict(end, env);
                false
            }
            Expr::Array { elements, .. } => {
                for element in elements {
                    self.strict(element, env);
                }
                false
            }
            Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
                for (_, value) in fields {
                    self.strict(value, env);
                }
                false
            }
            Expr::Interpolation { parts, .. } => {
                for part in parts {
                    if let InterpPart::Hole(hole) = part {
                        self.strict(hole, env);
                    }
                }
                false
            }
            // A lambda body result is strict (a closure never returns a promise). The outer
            // scope is visible so a captured deferred value used strictly inside is forced.
            Expr::Lambda { body, .. } => {
                self.strict(body, env);
                false
            }

            // Lazy carriers: the arms/result flow the value through without forcing, so the
            // If/Match/Block delivers deferred iff any branch does — the parent slot forces it.
            Expr::If {
                cond, then, else_, ..
            } => {
                self.strict(cond, env);
                let then_deferred = self.visit(then, env);
                let else_deferred = self.visit(else_, env);
                then_deferred || else_deferred
            }
            Expr::Match { expr, arms, .. } => {
                self.strict(expr, env);
                let mut any = false;
                for arm in arms {
                    any |= self.visit(&arm.body, env);
                }
                any
            }
            Expr::Block { stmts, .. } => self.visit_block(stmts, env),
        }
    }

    /// A block introduces a scope. Bindings carry their value's deferredness (a `=` is lazy);
    /// non-final statement values are discarded (not forced — the launch still runs); the
    /// final expression's value is the block's value, delivered to the block's own slot.
    fn visit_block(&mut self, stmts: &[Statement], env: &Scope) -> bool {
        let mut local = env.child();
        let last = stmts.len().saturating_sub(1);
        let mut result_deferred = false;
        for (index, stmt) in stmts.iter().enumerate() {
            match stmt {
                Statement::Item(Item::VarDecl(v)) => {
                    let deferred = self.visit(&v.value, &local);
                    local.bind(v.name.clone(), deferred);
                }
                Statement::Item(Item::FunctionDecl(f)) => {
                    self.strict(&f.body, &Scope::new());
                }
                Statement::Item(Item::TypeDecl(_)) => {}
                Statement::Expr(e) => {
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
/// from the map (params, pattern bindings from a forced scrutinee, globals) are ready.
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

/// Whether `func`/`args` is a call to the `@readStdin` primitive (`@readStdin()`, no arguments).
fn is_read_call(func: &Expr, args: &[Expr]) -> bool {
    matches!(func, Expr::Ident { name, .. } if name == READ_PRIMITIVE) && args.is_empty()
}

/// Apply `f` to `expr` and every sub-expression (pre-order). The one structural walk the
/// small analyses here share, so a new `Expr` variant is handled in one place.
fn for_each_subexpr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Number { .. }
        | Expr::String { .. }
        | Expr::Bool { .. }
        | Expr::Unit { .. }
        | Expr::Ident { .. } => {}
        Expr::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpPart::Hole(hole) = part {
                    for_each_subexpr(hole, f);
                }
            }
        }
        Expr::Call { func, args, .. } => {
            for_each_subexpr(func, f);
            for arg in args {
                for_each_subexpr(arg, f);
            }
        }
        Expr::BinOp { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            for_each_subexpr(left, f);
            for_each_subexpr(right, f);
        }
        Expr::Range { start, end, .. } => {
            for_each_subexpr(start, f);
            for_each_subexpr(end, f);
        }
        Expr::UnaryOp { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Spread { expr, .. }
        | Expr::Lambda { body: expr, .. } => for_each_subexpr(expr, f),
        Expr::FieldAssign { target, value, .. } => {
            for_each_subexpr(target, f);
            for_each_subexpr(value, f);
        }
        Expr::Index { expr, index, .. } => {
            for_each_subexpr(expr, f);
            for_each_subexpr(index, f);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            for_each_subexpr(cond, f);
            for_each_subexpr(then, f);
            for_each_subexpr(else_, f);
        }
        Expr::Match { expr, arms, .. } => {
            for_each_subexpr(expr, f);
            for arm in arms {
                for_each_subexpr(&arm.body, f);
            }
        }
        Expr::Array { elements, .. } => {
            for element in elements {
                for_each_subexpr(element, f);
            }
        }
        Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
            for (_, value) in fields {
                for_each_subexpr(value, f);
            }
        }
        Expr::Block { stmts, .. } => {
            for stmt in stmts {
                match stmt {
                    Statement::Expr(e) => for_each_subexpr(e, f),
                    Statement::Item(Item::VarDecl(v)) => for_each_subexpr(&v.value, f),
                    Statement::Item(Item::FunctionDecl(fun)) => for_each_subexpr(&fun.body, f),
                    Statement::Item(Item::TypeDecl(_)) => {}
                }
            }
        }
    }
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
    fn read_through_a_ternary_arm_forces_at_the_result_use() {
        // Ternary arms are lazy carriers: the deferred value survives the `?` and is forced
        // where the ternary's result is used strictly (the outer comparison).
        let src = "<< core.io\n^ = () -> Num => <\n  x = @readStdin()\n  chosen = true ? x : \"z\"\n  chosen == \"hi\" ? 0 : 1\n>";
        assert_eq!(force_count(src), 1);
    }
}
