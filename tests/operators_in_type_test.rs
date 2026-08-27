//! Operator overloading defined INSIDE the type. An operator is a member of the record
//! or sum type it operates on, with `it` the left operand and the one explicit parameter
//! the right. Sum types gain an optional trailing `{ }` block of methods (named methods,
//! operator members, and the render `` ` ``). Top-level operator definitions are rejected.
//!
//! Drives the full pipeline (lex -> parse -> typecheck -> codegen -> JIT), asserting the
//! entry point's exit code, plus the negative parse/type-checking cases.

mod common;
use common::{assert_exit, assert_exit_linked, assert_parse_error, assert_type_error};

// --- Record operator members ---

#[test]
fn record_equality_member_dispatches() {
    // `==` is a member of Color; `it` is the left operand, `other` the right.
    assert_exit(
        "Color = { r :: Num, g :: Num, == = (other :: Color) -> Bool => it.r == other.r && it.g == other.g }\n\
         ^ = () -> Num => <\n\
           a = Color { r = 1, g = 2 }\n\
           a == Color { r = 1, g = 2 } && !(a == Color { r = 9, g = 2 }) ? 42 : 0\n\
         >",
        42,
    );
}

#[test]
fn record_arithmetic_member_returns_the_record() {
    // `+` on Vec returns a Vec; the two components sum, read back after the call.
    assert_exit(
        "Vec = { x :: Num, y :: Num, + = (other :: Vec) -> Vec => Vec { x = it.x + other.x, y = it.y + other.y } }\n\
         ^ = () -> Num => <\n\
           v = Vec { x = 1, y = 2 } + Vec { x = 30, y = 9 }\n\
           v.x + v.y\n\
         >",
        42,
    );
}

#[test]
fn operator_member_dispatches_by_differing_operand_types() {
    // The right operand need not be the receiver's type: `Vec * Num -> Vec` scales.
    assert_exit(
        "Vec = { x :: Num, y :: Num, * = (k :: Num) -> Vec => Vec { x = it.x * k, y = it.y * k } }\n\
         ^ = () -> Num => <\n\
           v = Vec { x = 3, y = 4 } * 6\n\
           v.x + v.y\n\
         >",
        42,
    );
}

#[test]
fn record_render_member_is_used_by_interpolation_and_print() {
    assert_exit_linked(
        "<< core.test\n\
         Color = { r :: Num, g :: Num, ` = () -> Text => \"Color(`it.r`, `it.g`)\" }\n\
         ^ = () -> $ => assert(\"`Color { r = 1, g = 2 }`\", equals(\"Color(1, 2)\"))",
        0,
    );
}

// --- Sum { } method blocks ---

#[test]
fn sum_named_method_matches_on_it() {
    // A named method dispatched on the receiver's sum type; `it` is the whole value.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num) {\n\
           area = () -> Num => it ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h\n\
         }\n\
         ^ = () -> Num => Rect(6, 7).area()",
        42,
    );
}

#[test]
fn sum_equality_member_dispatches() {
    // A `==` member on a sum, resolved from Shape's methods (equal areas here).
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num) {\n\
           area = () -> Num => it ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h\n\
           == = (other :: Shape) -> Bool => it.area() == other.area()\n\
         }\n\
         ^ = () -> Num => Rect(6, 7) == Rect(2, 21) ? 42 : 0",
        42,
    );
}

#[test]
fn sum_render_member_is_used_by_interpolation() {
    assert_exit_linked(
        "<< core.test\n\
         Shape = Circle(Num) / Rect(Num, Num) {\n\
           ` = () -> Text => it ? | Circle(r) => \"Circle(`r`)\" | Rect(w, h) => \"Rect(`w`x`h`)\"\n\
         }\n\
         ^ = () -> $ => <\n\
           assert(\"`Rect(6, 7)`\", equals(\"Rect(6x7)\"))\n\
           assert(\"`Circle(4)`\", equals(\"Circle(4)\"))\n\
         >",
        0,
    );
}

#[test]
fn sum_render_member_rendering_it_wholesale_falls_back_to_the_variant_name() {
    // A sum `` ` `` override that renders its own receiver `it` wholesale must NOT recurse
    // forever — that one case uses the built-in default (the variant name), like a record's.
    assert_exit_linked(
        "<< core.test\n\
         Shape = Circle(Num) / Rect(Num, Num) {\n\
           ` = () -> Text => \"Shape: `it`\"\n\
         }\n\
         ^ = () -> $ => <\n\
           assert(\"`Circle(4)`\", equals(\"Shape: Circle\"))\n\
           assert(\"`Rect(6, 7)`\", equals(\"Shape: Rect\"))\n\
         >",
        0,
    );
}

#[test]
fn sum_with_no_method_block_is_unchanged() {
    // The `{ }` block is optional — a plain sum is written exactly as before.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         area = (s :: Shape) -> Num => s ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h\n\
         ^ = () -> Num => area(Rect(6, 7))",
        42,
    );
}

// --- Negative cases ---

#[test]
fn a_top_level_operator_definition_is_rejected() {
    // Operator overloading lives inside a type now; a top-level operator def is an error.
    assert_type_error(
        "Color = { r :: Num }\n== = (a :: Color, b :: Color) -> Bool => a.r == b.r\n^ = () -> Num => 0",
    );
}

#[test]
fn a_field_in_a_sum_method_block_is_rejected() {
    // A sum has no fields — a field-like entry inside its `{ }` block is a compile error.
    assert_parse_error("Shape = Circle(Num) / Rect(Num, Num) {\n  x :: Num\n}\n^ = () -> Num => 0");
}

#[test]
fn a_binary_operator_member_with_no_parameter_is_rejected() {
    // A binary operator member takes exactly one explicit parameter (the right operand).
    assert_type_error("V = { x :: Num, == = () -> Bool => true }\n^ = () -> Num => 0");
}

#[test]
fn a_comparison_member_returning_non_bool_is_rejected() {
    assert_type_error("V = { x :: Num, == = (other :: V) -> V => it }\n^ = () -> Num => 0");
}
