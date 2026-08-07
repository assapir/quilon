//! Free-variable / capture analysis for closures (M3).
//!
//! A lambda *captures* an identifier when its body references a name bound in an
//! enclosing scope (and not shadowed by one of the lambda's own bindings). Capture is
//! purely lexical; how each captured name is captured (by value vs by reference) is
//! decided by the binding operator at codegen time, not here.
//!
//! The one subtlety is that Quilon's `:=` is BOTH "mutable bind" and "reassign": inside a
//! closure, `x := v` *reassigns the captured cell* when `x` names an enclosing binding,
//! but *declares a fresh local* when it does not. So this analysis is parameterized by
//! the set of enclosing names (`outer`): a `:=` to an outer name is a use (capture), a
//! `:=` to a new name is a local. It never needs to resolve types.

use super::nodes::{Expr, ForPattern, Item, Pattern, Statement};
use std::collections::HashSet;

/// The ordered, de-duplicated names a lambda captures: references in its body to names in
/// `outer` (the enclosing scope) that the lambda has not shadowed with its own parameter
/// or local binding. `params` are the lambda's parameter names (which shadow `outer`).
/// Order follows first textual appearance, giving the closure environment a stable field
/// layout.
pub fn lambda_free_idents(params: &[String], body: &Expr, outer: &HashSet<String>) -> Vec<String> {
    // `local` accumulates names bound INSIDE the lambda (params first); a read or write of
    // a `local` name is never a capture. A name that is neither local nor outer is a
    // top-level/global reference, also not captured.
    let mut local: HashSet<String> = params.iter().cloned().collect();
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    collect(body, &mut local, outer, &mut seen, &mut ordered);
    ordered
}

/// Record a reference to `name` as a capture if it resolves to an enclosing binding
/// (`outer`) and is not locally shadowed.
fn note(
    name: &str,
    local: &HashSet<String>,
    outer: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    if !local.contains(name) && outer.contains(name) && seen.insert(name.to_string()) {
        out.push(name.to_string());
    }
}

fn collect(
    expr: &Expr,
    local: &mut HashSet<String>,
    outer: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    match expr {
        Expr::Ident { name, .. } => note(name, local, outer, seen, out),
        Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Unit { .. } => {}
        Expr::BinOp { left, right, .. }
        | Expr::Pipeline { left, right, .. }
        | Expr::Range {
            start: left,
            end: right,
            ..
        } => {
            collect(left, local, outer, seen, out);
            collect(right, local, outer, seen, out);
        }
        Expr::UnaryOp { expr, .. } | Expr::FieldAccess { expr, .. } => {
            collect(expr, local, outer, seen, out)
        }
        Expr::Call { func, args, .. } => {
            collect(func, local, outer, seen, out);
            for a in args {
                collect(a, local, outer, seen, out);
            }
        }
        Expr::Lambda { params, body, .. } => {
            // A nested lambda's parameters shadow within its own body; names it reads from
            // OUR scope are transitively free in us too. Its locals are its own — clone so
            // they don't leak back into ours.
            let mut inner = local.clone();
            for p in params {
                inner.insert(p.name.clone());
            }
            collect(body, &mut inner, outer, seen, out);
        }
        Expr::Block { stmts, .. } => {
            // A block opens a nested scope; thread a forward-growing local set through it.
            let mut block_local = local.clone();
            for stmt in stmts {
                match stmt {
                    Statement::Expr(e) => collect(e, &mut block_local, outer, seen, out),
                    Statement::Item(Item::VarDecl(decl)) => {
                        // The initializer runs BEFORE the name binds.
                        collect(&decl.value, &mut block_local, outer, seen, out);
                        // `x := v` where `x` is an outer binding not yet shadowed locally
                        // is a REASSIGNMENT of the captured cell — a use, so capture `x`
                        // and do NOT shadow it. Any other binding introduces a local.
                        let is_outer_reassign = decl.mutable
                            && !block_local.contains(&decl.name)
                            && outer.contains(&decl.name);
                        if is_outer_reassign {
                            note(&decl.name, &block_local, outer, seen, out);
                        } else {
                            block_local.insert(decl.name.clone());
                        }
                    }
                    Statement::Item(Item::FunctionDecl(decl)) => {
                        // A nested function is itself a closure: names it reads from OUR
                        // scope are transitively free in us too. Analyze its body with its
                        // parameters shadowing (a cloned local set), then bind its name.
                        let mut inner = block_local.clone();
                        for p in &decl.params {
                            inner.insert(p.name.clone());
                        }
                        collect(&decl.body, &mut inner, outer, seen, out);
                        block_local.insert(decl.name.clone());
                    }
                    Statement::Item(Item::TypeDecl(_)) => {}
                }
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect(cond, local, outer, seen, out);
            collect(then, local, outer, seen, out);
            collect(else_, local, outer, seen, out);
        }
        Expr::Match { expr, arms, .. } => {
            collect(expr, local, outer, seen, out);
            for arm in arms {
                let mut arm_local = local.clone();
                bind_pattern(&arm.pattern, &mut arm_local);
                collect(&arm.body, &mut arm_local, outer, seen, out);
            }
        }
        Expr::FieldAssign { target, value, .. } => {
            collect(target, local, outer, seen, out);
            collect(value, local, outer, seen, out);
        }
        Expr::Index { expr, index, .. } => {
            collect(expr, local, outer, seen, out);
            collect(index, local, outer, seen, out);
        }
        Expr::Array { elements, .. } => {
            for e in elements {
                collect(e, local, outer, seen, out);
            }
        }
        Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
            for (_, e) in fields {
                collect(e, local, outer, seen, out);
            }
        }
        Expr::SumConstructor { args, .. } => {
            for a in args {
                collect(a, local, outer, seen, out);
            }
        }
        Expr::ForLoop {
            collection,
            pattern,
            body,
            ..
        } => {
            collect(collection, local, outer, seen, out);
            let mut inner = local.clone();
            match pattern {
                ForPattern::Item { name, .. } => {
                    inner.insert(name.clone());
                }
                ForPattern::ItemIndex { item, index, .. } => {
                    inner.insert(item.clone());
                    inner.insert(index.clone());
                }
            }
            collect(body, &mut inner, outer, seen, out);
        }
    }
}

fn bind_pattern(pattern: &Pattern, bound: &mut HashSet<String>) {
    match pattern {
        Pattern::Ident { name, .. } => {
            bound.insert(name.clone());
        }
        Pattern::Constructor { args, .. } => {
            for a in args {
                bind_pattern(a, bound);
            }
        }
        Pattern::Number { .. } | Pattern::Wildcard { .. } => {}
    }
}
