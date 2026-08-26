// Shared structural traversal over expressions.

use crate::ast::{Expression, InterpolationPart, Item, Statement};

/// Apply `f` to `expression` and every sub-expression (pre-order).
///
/// The one structural walk the AST analyses share, so a new `Expression` variant is
/// handled in one place. The match is exhaustive with no catch-all: adding a variant
/// fails to compile here until it is given an arm, which is what keeps every caller
/// from silently ignoring it.
///
/// Descends into a nested function declaration's body, since that body is still code the
/// enclosing expression runs; it does not descend into item SIGNATURES or type declarations.
pub fn for_each_subexpression(expression: &Expression, f: &mut impl FnMut(&Expression)) {
    f(expression);
    match expression {
        Expression::Number { .. }
        | Expression::String { .. }
        | Expression::Bool { .. }
        | Expression::Unit { .. }
        | Expression::Identifier { .. } => {}
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Hole(hole) = part {
                    for_each_subexpression(hole, f);
                }
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            for_each_subexpression(function, f);
            for arg in arguments {
                for_each_subexpression(arg, f);
            }
        }
        Expression::BinaryOperator { left, right, .. }
        | Expression::Pipeline { left, right, .. } => {
            for_each_subexpression(left, f);
            for_each_subexpression(right, f);
        }
        Expression::Range { start, end, .. } => {
            for_each_subexpression(start, f);
            for_each_subexpression(end, f);
        }
        Expression::UnaryOperator { expression, .. }
        | Expression::FieldAccess { expression, .. }
        | Expression::Spread { expression, .. }
        | Expression::Lambda {
            body: expression, ..
        } => for_each_subexpression(expression, f),
        Expression::FieldAssign { target, value, .. } => {
            for_each_subexpression(target, f);
            for_each_subexpression(value, f);
        }
        Expression::Index {
            expression, index, ..
        } => {
            for_each_subexpression(expression, f);
            for_each_subexpression(index, f);
        }
        Expression::If {
            condition,
            then,
            else_,
            ..
        } => {
            for_each_subexpression(condition, f);
            for_each_subexpression(then, f);
            for_each_subexpression(else_, f);
        }
        Expression::Match {
            expression, arms, ..
        } => {
            for_each_subexpression(expression, f);
            for arm in arms {
                for_each_subexpression(&arm.body, f);
            }
        }
        Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
            for element in elements {
                for_each_subexpression(element, f);
            }
        }
        Expression::MapLiteral { entries, .. } => {
            for (key, value) in entries {
                for_each_subexpression(key, f);
                for_each_subexpression(value, f);
            }
        }
        Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
            for (_, value) in fields {
                for_each_subexpression(value, f);
            }
        }
        Expression::Block { statements, .. } => {
            for statement in statements {
                match statement {
                    Statement::Expression(e) => for_each_subexpression(e, f),
                    Statement::Item(Item::VariableDeclaration(v)) => {
                        for_each_subexpression(&v.value, f)
                    }
                    Statement::Item(Item::FunctionDeclaration(fun)) => {
                        for_each_subexpression(&fun.body, f)
                    }
                    Statement::Item(Item::TypeDeclaration(_)) => {}
                }
            }
        }
    }
}
