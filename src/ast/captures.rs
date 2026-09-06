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

use super::nodes::{Expression, InterpolationPart, Item, Pattern, Statement};
use std::collections::HashSet;

/// The ordered, de-duplicated names a lambda captures: references in its body to names in
/// `outer` (the enclosing scope) that the lambda has not shadowed with its own parameter
/// or local binding. `parameters` are the lambda's parameter names (which shadow `outer`).
/// Order follows first textual appearance, giving the closure environment a stable field
/// layout.
pub fn lambda_free_idents(
    parameters: &[String],
    body: &Expression,
    outer: &HashSet<String>,
) -> Vec<String> {
    // `local` accumulates names bound INSIDE the lambda (parameters first); a read or write of
    // a `local` name is never a capture. A name that is neither local nor outer is a
    // top-level/global reference, also not captured.
    let mut local: HashSet<String> = parameters.iter().cloned().collect();
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
    expression: &Expression,
    local: &mut HashSet<String>,
    outer: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    match expression {
        Expression::Identifier { name, .. } => note(name, local, outer, seen, out),
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. } => {}
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Hole(e) = part {
                    collect(e, local, outer, seen, out);
                }
            }
        }
        Expression::BinaryOperator { left, right, .. }
        | Expression::Range {
            start: left,
            end: right,
            ..
        } => {
            collect(left, local, outer, seen, out);
            collect(right, local, outer, seen, out);
        }
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. } => {
            collect(expression, local, outer, seen, out)
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect(function, local, outer, seen, out);
            for a in arguments {
                collect(a, local, outer, seen, out);
            }
        }
        Expression::Lambda {
            parameters, body, ..
        } => {
            // A nested lambda's parameters shadow within its own body; names it reads from
            // OUR scope are transitively free in us too. Its locals are its own — clone so
            // they don't leak back into ours.
            let mut inner = local.clone();
            for p in parameters {
                inner.insert(p.name.clone());
            }
            collect(body, &mut inner, outer, seen, out);
        }
        Expression::Block { statements, .. } => {
            // A block opens a nested scope; thread a forward-growing local set through it.
            let mut block_local = local.clone();
            for statement in statements {
                match statement {
                    Statement::Expression(e) => collect(e, &mut block_local, outer, seen, out),
                    Statement::Item(Item::VariableDeclaration(declaration)) => {
                        // The initializer runs BEFORE the name binds.
                        collect(&declaration.value, &mut block_local, outer, seen, out);
                        // `x := v` where `x` is an outer binding not yet shadowed locally
                        // is a REASSIGNMENT of the captured cell — a use, so capture `x`
                        // and do NOT shadow it. Any other binding introduces a local.
                        let is_outer_reassign = declaration.mutable
                            && !block_local.contains(&declaration.name)
                            && outer.contains(&declaration.name);
                        if is_outer_reassign {
                            note(&declaration.name, &block_local, outer, seen, out);
                        } else {
                            block_local.insert(declaration.name.clone());
                        }
                    }
                    Statement::Item(Item::FunctionDeclaration(declaration)) => {
                        // A nested function is itself a closure: names it reads from OUR
                        // scope are transitively free in us too. Analyze its body with its
                        // parameters shadowing (a cloned local set), then bind its name.
                        let mut inner = block_local.clone();
                        for p in &declaration.parameters {
                            inner.insert(p.name.clone());
                        }
                        collect(&declaration.body, &mut inner, outer, seen, out);
                        block_local.insert(declaration.name.clone());
                    }
                    Statement::Item(Item::TypeDeclaration(_)) => {}
                }
            }
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            collect(condition, local, outer, seen, out);
            collect(then, local, outer, seen, out);
            collect(else_, local, outer, seen, out);
        }
        Expression::Match {
            expression, arms, ..
        } => {
            collect(expression, local, outer, seen, out);
            for arm in arms {
                let mut arm_local = local.clone();
                bind_pattern(&arm.pattern, &mut arm_local);
                collect(&arm.body, &mut arm_local, outer, seen, out);
            }
        }
        Expression::FieldAssign { target, value, .. }
        | Expression::IndexAssign { target, value, .. } => {
            collect(target, local, outer, seen, out);
            collect(value, local, outer, seen, out);
        }
        Expression::Index {
            expression, index, ..
        } => {
            collect(expression, local, outer, seen, out);
            collect(index, local, outer, seen, out);
        }
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            for e in elements {
                collect(e, local, outer, seen, out);
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (k, v) in entries {
                collect(k, local, outer, seen, out);
                collect(v, local, outer, seen, out);
            }
        }
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            for (_, e) in fields {
                collect(e, local, outer, seen, out);
            }
        }
        Expression::Spread { expression, .. } => collect(expression, local, outer, seen, out),
    }
}

fn bind_pattern(pattern: &Pattern, bound: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier { name, .. } => {
            bound.insert(name.clone());
        }
        Pattern::Constructor { arguments, .. } => {
            for a in arguments {
                bind_pattern(a, bound);
            }
        }
        Pattern::Number { .. } | Pattern::Wildcard { .. } => {}
    }
}
