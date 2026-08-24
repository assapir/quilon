//! Which top-level functions a program can actually reach, so codegen can skip emitting
//! the rest.
//!
//! A single `<< core.test` pulls in every assertion the module defines; a program that
//! uses one of them still paid to emit — and, under `quilon run`, to JIT-compile — all of
//! them. Across the examples that is over half of every function emitted.
//!
//! The analysis is deliberately a coarse over-approximation: it collects **names
//! mentioned** anywhere in reachable code, without resolving them. A mention of `f` keeps
//! every top-level `f` (all members of an overload set, since a call resolves to one of
//! them by argument type, which this does not compute); an operator keeps the overload set
//! named with its symbol; a field access keeps a function that happens to share the field's
//! name; and a name shadowed by a local still keeps its top-level namesake. All of those
//! err towards emitting a function that is not needed, never towards dropping one that is.
//!
//! What is NOT pruned: type declarations and their methods, and top-level bindings. Their
//! bodies are therefore roots — a method that calls a helper keeps that helper.

use super::nodes::{BinOp, Expr, InterpPart, Item, Program, Statement, TypeDef, UnaryOp};
use std::collections::{HashMap, HashSet};

/// The names of every top-level function that program execution could reach, starting from
/// the `^` entry point.
///
/// `None` means "prune nothing": a program with no `^` is a module being compiled on its
/// own, where there is no entry point to be reachable from and every function is something
/// a later program might call.
pub fn reachable_functions(program: &Program) -> Option<HashSet<&str>> {
    if !program
        .items
        .iter()
        .any(|item| matches!(item, Item::FunctionDecl(decl) if decl.name == "^"))
    {
        return None;
    }

    // One pass over the items collects both halves of the problem: the roots — `^` plus
    // everything emitted unconditionally, which is every top-level binding's value and every
    // method body — and an index of function bodies by name. The index matters: looking a
    // name up by walking the item list would make the analysis quadratic in the number of
    // functions, costing more on a large program than the emission it saves.
    let mut pending: Vec<&str> = vec!["^"];
    let mut defined: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for item in &program.items {
        match item {
            Item::FunctionDecl(decl) => defined
                .entry(decl.name.as_str())
                .or_default()
                .push(&decl.body),
            Item::VarDecl(decl) => mentions(&decl.value, &mut pending),
            Item::TypeDecl(decl) => {
                if let TypeDef::Record { methods, .. } = &decl.type_def {
                    for method in methods {
                        mentions(&method.body, &mut pending);
                    }
                }
            }
        }
    }

    let mut reached: HashSet<&str> = HashSet::new();
    while let Some(name) = pending.pop() {
        if !reached.insert(name) {
            continue;
        }
        // Every top-level definition of this name — an overload set's members share it.
        if let Some(bodies) = defined.get(name) {
            for body in bodies {
                mentions(body, &mut pending);
            }
        }
    }
    Some(reached)
}

/// Push every name `expr` mentions onto `out`: identifiers, the symbols of the operators it
/// applies (an operator is an overload set named with its symbol), and the field/method
/// names it selects. Duplicates are fine — the caller de-duplicates as it walks. Names are
/// borrowed from the AST rather than copied: on a large program this walk sees hundreds of
/// thousands of mentions, and allocating for each one cost more than the emission it saves.
fn mentions<'a>(expr: &'a Expr, out: &mut Vec<&'a str>) {
    match expr {
        Expr::Ident { name, .. } => out.push(name),
        Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Unit { .. } => {}
        Expr::Interpolation { parts, .. } => {
            // Every hole renders through the `` ` `` operator.
            out.push("`");
            for part in parts {
                if let InterpPart::Hole(e) = part {
                    mentions(e, out);
                }
            }
        }
        Expr::BinOp {
            left, op, right, ..
        } => {
            out.push(op.symbol());
            mentions(left, out);
            mentions(right, out);
        }
        Expr::UnaryOp { op, expr, .. } => {
            if matches!(op, UnaryOp::Neg) {
                out.push(BinOp::Sub.symbol());
            }
            mentions(expr, out);
        }
        Expr::Pipeline { left, right, .. }
        | Expr::Range {
            start: left,
            end: right,
            ..
        }
        | Expr::FieldAssign {
            target: left,
            value: right,
            ..
        }
        | Expr::Index {
            expr: left,
            index: right,
            ..
        } => {
            mentions(left, out);
            mentions(right, out);
        }
        Expr::FieldAccess { expr, field, .. } => {
            // A method call is a call of a field access, so the method's name arrives here.
            out.push(field);
            mentions(expr, out);
        }
        Expr::Call {
            function,
            arguments,
            ..
        } => {
            mentions(function, out);
            for a in arguments {
                mentions(a, out);
            }
        }
        Expr::Lambda { body, .. } => mentions(body, out),
        Expr::Block { stmts, .. } => {
            for stmt in stmts {
                match stmt {
                    Statement::Expr(e) => mentions(e, out),
                    Statement::Item(Item::VarDecl(decl)) => mentions(&decl.value, out),
                    Statement::Item(Item::FunctionDecl(decl)) => mentions(&decl.body, out),
                    Statement::Item(Item::TypeDecl(decl)) => {
                        if let TypeDef::Record { methods, .. } = &decl.type_def {
                            for method in methods {
                                mentions(&method.body, out);
                            }
                        }
                    }
                }
            }
        }
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            mentions(condition, out);
            mentions(then, out);
            mentions(else_, out);
        }
        Expr::Match { expr, arms, .. } => {
            mentions(expr, out);
            for arm in arms {
                mentions(&arm.body, out);
            }
        }
        Expr::Array { elements, .. } => {
            for e in elements {
                mentions(e, out);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                mentions(key, out);
                mentions(value, out);
            }
        }
        Expr::SetLiteral { elements, .. } => {
            for e in elements {
                mentions(e, out);
            }
        }
        Expr::Record { fields, .. } => {
            for (_, e) in fields {
                mentions(e, out);
            }
        }
        Expr::Constructor {
            type_name, fields, ..
        } => {
            out.push(type_name);
            for (_, e) in fields {
                mentions(e, out);
            }
        }
        Expr::Spread { expr, .. } => mentions(expr, out),
    }
}
