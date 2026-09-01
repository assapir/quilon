//! Top-level bindings: what a global may hold, and what happens to one that would have
//! to be computed.
//!
//! A global's initializer has to be a constant, and no Quilon code runs before `^` in
//! which to compute one. The rejection is the interesting half: it used to pass
//! `quilon check` and then fail inside codegen, which built the value's instructions with
//! the builder still pointing at the last function it had emitted — an internal
//! `UnsetPosition` error for an operator, and for a call a module whose verification
//! failed because a block had been left without a terminator.

mod common;
use common::{assert_exit, assert_type_error};

#[test]
fn a_literal_or_function_global_is_accepted() {
    assert_exit("limit = 10\n^ = () -> Num => < limit >", 10);
    assert_exit("on = true\n^ = () -> Num => < on ? 3 : 4 >", 3);
    assert_exit("nothing = $\n^ = () -> Num => < 7 >", 7);
    assert_exit(
        "scale = (n :: Num) => < n * 3 >\n^ = () -> Num => < scale(4) >",
        12,
    );
    assert_exit("a = 2\nb = 3\n^ = () -> Num => < a + b >", 5);
}

#[test]
fn a_mutable_global_is_written_through_from_a_function() {
    assert_exit(
        "counter := 4\n^ = () -> Num => <\n  counter := counter + 1\n  counter\n>",
        5,
    );
}

#[test]
fn a_computed_global_is_rejected_with_its_own_error() {
    // An operator: reached codegen as `Failed to build add: UnsetPosition`.
    assert_type_error("total = 1 + 2\n^ = () -> Num => < total >");
    // A call: reached codegen as a module that failed verification.
    assert_type_error("f = (n :: Num) -> Num => < n + 1 >\nx = f(1)\n^ = () -> Num => < x >");
    // A negated literal is still an operator applied to one.
    assert_type_error("x = -5\n^ = () -> Num => < x >");
    // Reading another global is a load, which is also work.
    assert_type_error("a = 1\nb = a\n^ = () -> Num => < b >");
}

#[test]
fn a_global_of_a_composite_type_is_rejected_the_same_way() {
    // `Text`, arrays, records and sum values are all built at runtime, so none of them
    // can initialize a global — including a bare `Text` literal, which is a
    // `{ pointer, length }` pair rather than a constant scalar.
    assert_type_error("greeting = \"hi\"\n^ = () -> Num => < 0 >");
    assert_type_error("xs = [1, 2]\n^ = () -> Num => < 0 >");
    assert_type_error("p = { x = 1 }\n^ = () -> Num => < 0 >");
    assert_type_error("r = Ok(1)\n^ = () -> Num => < 0 >");
}

#[test]
fn a_computed_global_inside_a_function_is_still_fine() {
    // The restriction is about globals only: the same expressions are ordinary work when
    // they are bound inside a function.
    assert_exit(
        "^ = () -> Num => <\n  total = 1 + 2\n  xs = [1, 2]\n  total + xs.size\n>",
        5,
    );
}
