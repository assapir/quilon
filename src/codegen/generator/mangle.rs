//! Symbol mangling: the distinct LLVM symbol each overload member and method is
//! emitted under, built from its name and parameter types so a call site mangles to
//! the same symbol the definition did.

use super::*;

/// A short, mangling-safe tag for a Quilon type used in overload name mangling. Must be
/// deterministic and identical at definition and call sites (built from the declared
/// parameter type and from the inferred argument type respectively).
pub(super) fn type_mangle(ty: &Type) -> String {
    match ty {
        Type::Num => "N".to_string(),
        Type::Text => "T".to_string(),
        Type::Bool => "B".to_string(),
        Type::Unit => "U".to_string(),
        Type::Array(elem) => format!("A{}", type_mangle(elem)),
        Type::Named { name, .. } | Type::Sum { name, .. } => format!("named${}", name),
        // A not-yet-concrete sum payload (`Generic`) resolves as `Num` for overload
        // dispatch (see the type checker's `types_match`), so it mangles to the Num tag
        // — keeping codegen's chosen symbol in agreement with the checker.
        Type::Generic { .. } => "N".to_string(),
        // Any other shape (e.g. a function type) — a stable, mangling-safe fallback.
        other => format!("X{:?}", other)
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '$')
            .collect(),
    }
}

/// Render an entry point's declared parameter types as a readable signature fragment
/// (comma-joined `Num`/`Text`/`[]Text`-style labels) for the unsupported-signature
/// diagnostic. `()` renders as an empty string. Uses the shared `ast::type_label` so
/// codegen and the type checker render types identically.
pub(super) fn fmt_parameter_types(parameters: &[Type]) -> String {
    parameters
        .iter()
        .map(crate::ast::type_label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The distinct LLVM symbol for one overload member: its name plus a per-parameter
/// type tag. Operator symbols (which aren't valid LLVM identifiers) are spelled out so
/// e.g. `+` on `(Point, Point)` becomes `operator.add$named$Point$named$Point`.
pub(super) fn mangle_overload(name: &str, parameters: &[Type]) -> String {
    let base = operator_word(name)
        .map(|w| format!("op.{}", w))
        .unwrap_or_else(|| name.to_string());
    let mut s = base;
    for p in parameters {
        s.push('$');
        s.push_str(&type_mangle(p));
    }
    s
}

/// The LLVM symbol for a record method `Type.method`. The render operator `` ` `` is not a
/// valid identifier, so it is spelled out (`Type_op$backtick`); every other method name is
/// used verbatim. Shared by method declaration, body emission, and call dispatch so all
/// three always agree.
pub(super) fn method_symbol(type_name: &str, method_name: &str) -> String {
    let m = if method_name == "`" {
        "op$backtick"
    } else {
        method_name
    };
    format!("{}_{}", type_name, m)
}

/// A pronounceable word for an operator symbol, for use in a mangled LLVM name (which
/// can't contain the raw symbol). Returns `None` for non-operator (ordinary) names.
pub(super) fn operator_word(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "%" => "mod",
        "==" => "eq",
        "!=" => "ne",
        "<" => "lt",
        "<=" => "le",
        ">" => "gt",
        ">=" => "ge",
        "&&" => "and",
        "||" => "or",
        _ => return None,
    })
}
