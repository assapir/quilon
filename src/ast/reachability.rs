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

use super::nodes::{
    BinaryOperator, Expression, InterpolationPart, Item, Program, Statement, UnaryOperator,
};
use std::collections::{HashMap, HashSet};

/// The names of every top-level function that program execution could reach, starting from
/// the `^` entry point.
///
/// `None` means "prune nothing": a program with no `^` is a module being compiled on its
/// own, where there is no entry point to be reachable from and every function is something
/// a later program might call.
pub fn reachable_functions(program: &Program) -> Option<HashSet<&str>> {
    if !program.items.iter().any(
        |item| matches!(item, Item::FunctionDeclaration(declaration) if declaration.name == "^"),
    ) {
        return None;
    }

    // One pass over the items collects both halves of the problem: the roots — `^` plus
    // everything emitted unconditionally, which is every top-level binding's value and every
    // method body — and an index of function bodies by name. The index matters: looking a
    // name up by walking the item list would make the analysis quadratic in the number of
    // functions, costing more on a large program than the emission it saves.
    let mut pending: Vec<&str> = vec!["^"];
    let mut defined: HashMap<&str, Vec<&Expression>> = HashMap::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => defined
                .entry(declaration.name.as_str())
                .or_default()
                .push(&declaration.body),
            Item::VariableDeclaration(declaration) => mentions(&declaration.value, &mut pending),
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    mentions(&method.body, &mut pending);
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

/// Push every name `expression` mentions onto `out`: identifiers, the symbols of the operators it
/// applies (an operator is an overload set named with its symbol), and the field/method
/// names it selects. Duplicates are fine — the caller de-duplicates as it walks. Names are
/// borrowed from the AST rather than copied: on a large program this walk sees hundreds of
/// thousands of mentions, and allocating for each one cost more than the emission it saves.
fn mentions<'a>(expression: &'a Expression, out: &mut Vec<&'a str>) {
    match expression {
        Expression::Identifier { name, .. } => out.push(name),
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. } => {}
        Expression::Interpolation { parts, .. } => {
            // Every hole renders through the `` ` `` operator.
            out.push("`");
            for part in parts {
                if let InterpolationPart::Hole(e) = part {
                    mentions(e, out);
                }
            }
        }
        Expression::BinaryOperator {
            left,
            operator,
            right,
            ..
        } => {
            out.push(operator.symbol());
            mentions(left, out);
            mentions(right, out);
        }
        Expression::UnaryOperator {
            operator,
            expression,
            ..
        } => {
            if matches!(operator, UnaryOperator::Neg) {
                out.push(BinaryOperator::Sub.symbol());
            }
            mentions(expression, out);
        }
        Expression::Pipeline { left, right, .. }
        | Expression::Range {
            start: left,
            end: right,
            ..
        }
        | Expression::FieldAssign {
            target: left,
            value: right,
            ..
        }
        | Expression::Index {
            expression: left,
            index: right,
            ..
        } => {
            mentions(left, out);
            mentions(right, out);
        }
        Expression::FieldAccess {
            expression, field, ..
        } => {
            // A method call is a call of a field access, so the method's name arrives here.
            out.push(field);
            mentions(expression, out);
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            mentions(function, out);
            for a in arguments {
                mentions(a, out);
            }
        }
        Expression::Lambda { body, .. } => mentions(body, out),
        Expression::Block { statements, .. } => {
            for statement in statements {
                match statement {
                    Statement::Expression(e) => mentions(e, out),
                    Statement::Item(Item::VariableDeclaration(declaration)) => {
                        mentions(&declaration.value, out)
                    }
                    Statement::Item(Item::FunctionDeclaration(declaration)) => {
                        mentions(&declaration.body, out)
                    }
                    Statement::Item(Item::TypeDeclaration(declaration)) => {
                        for method in declaration.type_definition.methods() {
                            mentions(&method.body, out);
                        }
                    }
                }
            }
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            mentions(condition, out);
            mentions(then, out);
            mentions(else_, out);
        }
        Expression::Match {
            expression, arms, ..
        } => {
            mentions(expression, out);
            for arm in arms {
                mentions(&arm.body, out);
            }
        }
        Expression::Array { elements, .. } => {
            for e in elements {
                mentions(e, out);
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                mentions(key, out);
                mentions(value, out);
            }
        }
        Expression::SetLiteral { elements, .. } => {
            for e in elements {
                mentions(e, out);
            }
        }
        Expression::Record { fields, .. } => {
            for (_, e) in fields {
                mentions(e, out);
            }
        }
        Expression::Constructor {
            type_name, fields, ..
        } => {
            out.push(type_name);
            for (_, e) in fields {
                mentions(e, out);
            }
        }
        Expression::Spread { expression, .. } => mentions(expression, out),
    }
}
