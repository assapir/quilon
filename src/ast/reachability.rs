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
//!
//! The one name the over-approximation does NOT stretch to is the receiver, `it`: it is bound
//! by the method it appears in, so a bare `it` is not a mention. An `it` in CALLEE position —
//! the one place it can name a top-level function, `core.test`'s case function — is
//! (see `mentions_callee`).

use super::nodes::{
    BinaryOperator, Expression, InterpolationPart, Item, Program, RECEIVER, Statement,
    UnaryOperator,
};
use std::collections::{HashMap, HashSet};

/// The names of every top-level function that program execution could reach, starting from
/// the `^` entry point plus every `>>`-exported top-level function OF THE PROGRAM'S OWN
/// FILES — a module's public surface is reachable by definition, whether or not this
/// program happens to call it itself, since an importer elsewhere might.
///
/// A corelib module's own exports (`from_corelib`) do NOT seed this on their own: a
/// linked program carries the whole of every corelib module one of its calls pulled in
/// (`core.text`'s composables, `core.test`'s harness, …), and the corelib's `>>` there
/// marks what a PROGRAM may reach through it, not "always emit this" — a corelib function
/// nothing reachable actually calls stays pruned exactly as before, so pulling in one
/// composable Text method does not drag in the rest of `core.text` (or, transitively,
/// `core.test`'s harness, which `core.text`'s own suite imports) along with it.
///
/// `None` means "prune nothing": with no `^` and no export of the program's own, nothing
/// here is reachable from anything this analysis can see, which for a module compiled on
/// its own (no public surface at all yet) means every function is something a later
/// program might still call, not that all of it is dead.
pub fn reachable_functions(program: &Program) -> Option<HashSet<&str>> {
    // Whether there is a root at all — `^`, or an export of the program's own — decided
    // up front and independently of the pass below: that pass pushes onto `pending`
    // unconditionally for a method body or a top-level binding's value too (both are
    // roots regardless of `^`/export), so an empty-`pending` check taken only after it
    // runs would wrongly see a root in a `^`-less, export-less module that merely has a
    // method or a computed global — and prune the rest of it, rather than keeping
    // everything the way no reachable root at all calls for.
    let has_root = program.items.iter().any(|item| {
        matches!(item, Item::FunctionDeclaration(declaration)
            if declaration.name == "^" || (declaration.exported && !declaration.from_corelib))
    });
    if !has_root {
        return None;
    }

    // One pass over the items collects both halves of the problem: the roots — `^`, every
    // export of the program's own, and everything emitted unconditionally, which is every
    // top-level binding's value and every method body — and an index of function bodies by
    // name. The index matters: looking a name up by walking the item list would make the
    // analysis quadratic in the number of functions, costing more on a large program than
    // the emission it saves.
    let mut pending: Vec<&str> = Vec::new();
    if program.items.iter().any(
        |item| matches!(item, Item::FunctionDeclaration(declaration) if declaration.name == "^"),
    ) {
        pending.push("^");
    }
    let mut defined: HashMap<&str, Vec<&Expression>> = HashMap::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(declaration) => {
                if declaration.exported && !declaration.from_corelib {
                    pending.push(declaration.name.as_str());
                }
                defined
                    .entry(declaration.name.as_str())
                    .or_default()
                    .push(&declaration.body);
            }
            Item::VariableDeclaration(declaration) => mentions(&declaration.value, &mut pending),
            Item::TypeDeclaration(declaration) => {
                for method in declaration.type_definition.methods() {
                    mentions(&method.body, &mut pending);
                }
            }
            // The runtime calls an arm directly, with no call site to mention it.
            Item::TrapDeclaration(trap) => {
                for arm in &trap.arms {
                    mentions(&arm.body, &mut pending);
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

/// The names mentioned anywhere in `expressions` — the same over-approximate walk
/// [`reachable_functions`] uses, without treating any of them as a root or following them
/// transitively. `run`/`build`/`check` erase a program's `test.describe` blocks before
/// compiling it (only `quilon test`'s synthesized `^` runs them — see `driver.rs`), so a
/// name only these blocks mention is not itself alive; the checker's dead-function check
/// reads this to tell that case apart from a function nothing anywhere reaches.
pub fn names_mentioned(expressions: &[Expression]) -> HashSet<&str> {
    let mut pending = Vec::new();
    for expression in expressions {
        mentions(expression, &mut pending);
    }
    pending.into_iter().collect()
}

/// Push every name `expression` mentions onto `out`: identifiers, the symbols of the operators it
/// applies (an operator is an overload set named with its symbol), and the field/method
/// names it selects. Duplicates are fine — the caller de-duplicates as it walks. Names are
/// borrowed from the AST rather than copied: on a large program this walk sees hundreds of
/// thousands of mentions, and allocating for each one cost more than the emission it saves.
/// [`mentions`] of an expression in CALLEE position — the `function` of a call, or the right
/// side of a pipeline, which desugars to one. This is the one place a bare `it` names a
/// top-level function rather than a method's receiver, since a receiver is not callable.
fn mentions_callee<'a>(callee: &'a Expression, out: &mut Vec<&'a str>) {
    match callee {
        Expression::Identifier { name, .. } if name == RECEIVER => out.push(name),
        other => mentions(other, out),
    }
}

fn mentions<'a>(expression: &'a Expression, out: &mut Vec<&'a str>) {
    match expression {
        Expression::Identifier { name, .. } if name == RECEIVER => {}
        Expression::Identifier { name, .. } => {
            out.push(name);
            // A Text-method mention reaches the `core.text` function that implements it:
            // `t.trim()` lowers to a call of `core.text.trim`, whose name appears nowhere
            // in the source. Over-approximate like everything here — any mention of `trim`
            // keeps the implementation, Text receiver or not.
            if let Some(implementation) = crate::ast::qn_text_impl(name) {
                out.push(implementation);
            }
        }
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
        Expression::Range {
            start: left,
            end: right,
            ..
        }
        | Expression::FieldAssign {
            target: left,
            value: right,
            ..
        }
        | Expression::IndexAssign {
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
            mentions_callee(function, out);
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
                    // A trap is a top-level-only item; never a block statement.
                    Statement::Item(Item::TrapDeclaration(_)) => {}
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
