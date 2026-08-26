// Shared structural traversal over expressions.

use crate::ast::{Expression, InterpolationPart, Item, Statement};
use std::ops::ControlFlow;

/// Apply `f` to `expression` and every sub-expression (pre-order), stopping early the
/// moment `f` returns [`ControlFlow::Break`].
///
/// The one structural walk the AST analyses share, so a new `Expression` variant is
/// handled in one place. The match is exhaustive with no catch-all: adding a variant
/// fails to compile here until it is given an arm, which is what keeps every caller
/// from silently ignoring it.
///
/// Descends into a nested function declaration's body, since that body is still code the
/// enclosing expression runs; it does not descend into item SIGNATURES or type declarations.
///
/// Use [`for_each_subexpression`] when the walk always visits everything; a search wants
/// this form, so it does not keep traversing after it has its answer.
pub fn try_for_each_subexpression(
    expression: &Expression,
    f: &mut impl FnMut(&Expression) -> ControlFlow<()>,
) -> ControlFlow<()> {
    f(expression)?;
    match expression {
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. }
        | Expression::Identifier { .. } => {}
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Hole(hole) = part {
                    try_for_each_subexpression(hole, f)?;
                }
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            try_for_each_subexpression(function, f)?;
            for arg in arguments {
                try_for_each_subexpression(arg, f)?;
            }
        }
        Expression::BinaryOperator { left, right, .. }
        | Expression::Pipeline { left, right, .. } => {
            try_for_each_subexpression(left, f)?;
            try_for_each_subexpression(right, f)?;
        }
        Expression::Range { start, end, .. } => {
            try_for_each_subexpression(start, f)?;
            try_for_each_subexpression(end, f)?;
        }
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. }
        | Expression::Spread { expression, .. }
        | Expression::Lambda {
            body: expression, ..
        } => try_for_each_subexpression(expression, f)?,
        Expression::FieldAssign { target, value, .. } => {
            try_for_each_subexpression(target, f)?;
            try_for_each_subexpression(value, f)?;
        }
        Expression::Index {
            expression, index, ..
        } => {
            try_for_each_subexpression(expression, f)?;
            try_for_each_subexpression(index, f)?;
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            try_for_each_subexpression(condition, f)?;
            try_for_each_subexpression(then, f)?;
            try_for_each_subexpression(else_, f)?;
        }
        Expression::Match {
            expression, arms, ..
        } => {
            try_for_each_subexpression(expression, f)?;
            for arm in arms {
                try_for_each_subexpression(&arm.body, f)?;
            }
        }
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            for element in elements {
                try_for_each_subexpression(element, f)?;
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                try_for_each_subexpression(key, f)?;
                try_for_each_subexpression(value, f)?;
            }
        }
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            for (_, value) in fields {
                try_for_each_subexpression(value, f)?;
            }
        }
        Expression::Block { statements, .. } => {
            for statement in statements {
                match statement {
                    Statement::Expression(e) => try_for_each_subexpression(e, f)?,
                    Statement::Item(Item::VariableDeclaration(v)) => {
                        try_for_each_subexpression(&v.value, f)?
                    }
                    Statement::Item(Item::FunctionDeclaration(fun)) => {
                        try_for_each_subexpression(&fun.body, f)?
                    }
                    Statement::Item(Item::TypeDeclaration(_)) => {}
                }
            }
        }
    }
    ControlFlow::Continue(())
}

/// Apply `f` to `expression` and every sub-expression (pre-order), visiting all of them.
/// The always-visit form of [`try_for_each_subexpression`].
pub fn for_each_subexpression(expression: &Expression, f: &mut impl FnMut(&Expression)) {
    let _ = try_for_each_subexpression(expression, &mut |e| {
        f(e);
        ControlFlow::Continue(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser;

    /// Parse `src` as a program and hand back its entry point's body.
    fn entry_body(src: &str) -> Expression {
        let tokens = Lexer::tokenize(src).expect("lexing failed");
        let program = parser::parse(&tokens).expect("parsing failed");
        program
            .items
            .into_iter()
            .find_map(|item| match item {
                Item::FunctionDeclaration(f) if f.name == "^" => Some(f.body),
                _ => None,
            })
            .expect("program has an entry point")
    }

    #[test]
    fn the_walk_stops_at_the_first_break() {
        // A search must not keep traversing once it has its answer. Breaking at the root
        // visits exactly one node, however large the body is — the property a predicate
        // like the checker's setter detection relies on to stay cheap on a long method
        // whose write sits in its first statement.
        let body = entry_body(
            "^ = () -> Num => <\n  a = [1, 2, 3].map(x => x + 1)\n  b = a.size + 2\n  b\n>",
        );
        let mut visited = 0;
        let outcome = try_for_each_subexpression(&body, &mut |_| {
            visited += 1;
            ControlFlow::Break(())
        });
        assert!(outcome.is_break(), "the break must reach the caller");
        assert_eq!(visited, 1, "the walk continued past a Break");
    }

    #[test]
    fn the_walk_visits_every_sub_expression_when_nothing_breaks() {
        // The counterpart: with no break, every node is visited — including the ones
        // nested in a lambda body and in a locally declared function's body, which are
        // the forms a hand-rolled walk is most likely to miss.
        let body = entry_body(
            "^ = () -> Num => <\n  helper = () -> Num => 7\n  a = [1].map(x => x + 1)\n  helper()\n>",
        );
        let mut numbers = Vec::new();
        for_each_subexpression(&body, &mut |e| {
            if let Expression::Number { value, .. } = e {
                numbers.push(*value);
            }
        });
        numbers.sort_by(f64::total_cmp);
        assert_eq!(
            numbers,
            vec![1.0, 1.0, 7.0],
            "every literal must be visited: 7 inside the declared function, and both \
             inside the lambda-bearing array expression"
        );
    }
}
