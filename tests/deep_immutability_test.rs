// Deep immutability: `=` freezes the VALUE, not just the binding. A value reached
// through an `=` binding is never reachable through a `:=` binding — in either
// direction — so every aliasing route around the setter/field-write gates is a compile
// error, while immutable aliases, matching-mutability stores, and fresh values stay
// legal. Scalars (`Num`/`Bool`/`Text`) copy and are exempt.

use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;

mod common;
use common::{assert_exit, type_error_message};

/// Assert the type checker ACCEPTS `src` — for legal forms whose run behavior is not the
/// point (or not yet lowered by codegen).
fn assert_type_checks(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    if let Err(error) = TypeChecker::new().check_program(&program) {
        panic!("expected the program to type-check, got: {error}\nsource:\n{src}");
    }
}

// --- Route 1: alias bindings. ---

#[test]
fn a_mutable_alias_of_an_immutable_record_is_rejected() {
    // `a := t` would make `t`'s frozen value writable through `a`.
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  t = T { v = 1 }\n  a := t\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the alias binding to name 't', got: {error}"
    );
}

#[test]
fn an_immutable_alias_of_a_mutable_record_is_rejected() {
    // The other direction: `x = m` would freeze a value that writes through `m` keep
    // changing underneath.
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  m := T { v = 1 }\n  x = m\n  m.v\n>",
    );
    assert!(
        error.contains("'m' is mutable"),
        "expected the alias binding to name 'm', got: {error}"
    );
}

#[test]
fn run_an_immutable_alias_of_an_immutable_record_stays_legal() {
    // `a = t` adds a second frozen name for the same frozen value: allowed, and the
    // value stays what it was.
    assert_exit(
        "T = { v :: Num }\n^ = () -> Num => <\n  t = T { v = 40 }\n  a = t\n  a.v + (t.v == 40 ? 2 : 0)\n>",
        42,
    );
}

#[test]
fn run_a_scalar_read_out_of_a_frozen_record_copies() {
    // Scalars copy: a field read into a `:=` binding takes the value out, and writing
    // the copy leaves the record untouched.
    assert_exit(
        "T = { v :: Num }\n^ = () -> Num => <\n  t = T { v = 40 }\n  y := t.v\n  y := y + 2\n  y + (t.v == 40 ? 0 : 100)\n>",
        42,
    );
}

// --- Route 2: a method result that aliases its receiver. ---

#[test]
fn a_mutable_binding_of_an_escaping_getter_result_is_rejected() {
    // `self` returns `it`, so its result IS the receiver: on an `=` receiver the result
    // is immutable at the call site, and `x := t.self()` is the alias binding again.
    let error = type_error_message(
        "T = {\n  v :: Num\n  self = () -> T => < it >\n}\n^ = () -> Num => <\n  t = T { v = 1 }\n  x := t.self()\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the escaping result to inherit 't''s immutability, got: {error}"
    );
}

#[test]
fn an_escaping_getter_stays_callable_on_an_immutable_receiver() {
    // The method itself stays callable: its result and scalar reads off it are legal.
    assert_type_checks(
        "T = {\n  v :: Num\n  self = () -> T => < it >\n}\n^ = () -> Num => <\n  t = T { v = 1 }\n  x = t.self()\n  y = t.self().v\n  x.v + y\n>",
    );
}

#[test]
fn run_an_escaping_getter_result_on_an_immutable_receiver_reads_back() {
    // `x = t.self()` is an immutable alias of `t`; reads work and `t` is unchanged.
    assert_exit(
        "T = {\n  v :: Num\n  self = () -> T => < it >\n}\n^ = () -> Num => <\n  t = T { v = 21 }\n  x = t.self()\n  x.v + t.v\n>",
        42,
    );
}

#[test]
fn run_an_escaping_getter_result_on_a_mutable_receiver_stays_mutable() {
    // On a `:=` receiver the escaping result inherits mutability: `z := m.self()` is
    // legal and a write through `z` reaches `m` — that is what `:=` declared.
    assert_exit(
        "T = {\n  v :: Num\n  self = () -> T => < it >\n}\n^ = () -> Num => <\n  m := T { v = 5 }\n  z := m.self()\n  z.v := 42\n  m.v\n>",
        42,
    );
}

// --- Route 3: containers, in both directions. ---

#[test]
fn storing_an_immutable_record_in_a_mutable_container_is_rejected() {
    // `b := Box { item = t }` would reach `t`'s frozen value through `b.item :=` writes.
    let error = type_error_message(
        "T = { v :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  t = T { v = 1 }\n  b := Box { item = t }\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the container store to name 't', got: {error}"
    );
}

#[test]
fn storing_a_mutable_record_in_an_immutable_container_is_rejected() {
    // Symmetric: `c = Box { item = m }` would freeze a container whose content keeps
    // changing through `m`.
    let error = type_error_message(
        "T = { v :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  m := T { v = 1 }\n  c = Box { item = m }\n  m.v\n>",
    );
    assert!(
        error.contains("'m' is mutable"),
        "expected the container store to name 'm', got: {error}"
    );
}

#[test]
fn storing_an_immutable_record_in_a_mutable_array_is_rejected() {
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  t = T { v = 1 }\n  arr := [t]\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the array store to name 't', got: {error}"
    );
}

#[test]
fn storing_a_mutable_record_in_an_immutable_array_is_rejected() {
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  m := T { v = 1 }\n  arr = [m]\n  m.v\n>",
    );
    assert!(
        error.contains("'m' is mutable"),
        "expected the array store to name 'm', got: {error}"
    );
}

#[test]
fn matching_mutability_stores_stay_legal() {
    // `=` into `=`, `:=` into `:=`: both directions of the matching case pass the
    // checker. (Running a record-typed field is a separate, pre-existing codegen gap —
    // record fields of user types are a deferred follow-up.)
    assert_type_checks(
        "T = { v :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  t = T { v = 40 }\n  frozen = Box { item = t }\n  m := T { v = 1 }\n  open := Box { item = m }\n  m.v := 2\n  t.v + m.v\n>",
    );
}

#[test]
fn reading_a_record_out_of_an_immutable_container_yields_an_immutable_result() {
    // Deep: what comes OUT of a frozen container is frozen too, for a field read and
    // for an element read.
    let error = type_error_message(
        "T = { v :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  t = T { v = 1 }\n  b = Box { item = t }\n  x := b.item\n  t.v\n>",
    );
    assert!(
        error.contains("is immutable"),
        "expected the field read to stay frozen, got: {error}"
    );

    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  arr = [T { v = 1 }]\n  x := arr[0]\n  1\n>",
    );
    assert!(
        error.contains("'arr' is immutable"),
        "expected the element read to stay frozen, got: {error}"
    );
}

#[test]
fn a_frozen_record_reached_through_a_sum_stays_frozen() {
    // A record wrapped in a sum payload and matched back out is still the same value:
    // a function returning the payload returns its parameter's value, so the call's
    // result inherits the argument's immutability.
    let error = type_error_message(
        "T = { v :: Num }\nWrap = Held(T) / Empty\nunwrap = (s :: Wrap) -> T => < s ? | Held(p) => p | Empty => T { v = 0 } >\n^ = () -> Num => <\n  t = T { v = 1 }\n  w := unwrap(Held(t))\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the matched-out payload to stay frozen, got: {error}"
    );
}

// --- Route 4: method-internal aliasing of the receiver. ---

#[test]
fn a_mutable_alias_of_the_receiver_inside_an_immutable_method_is_rejected() {
    // The original `sneak`: an `=` method must not let `it` reach a mutable binding —
    // the receiver may be `=`-bound at any call site.
    let error = type_error_message(
        "T = {\n  v :: Num\n  sneak = () -> Num => <\n    a := it\n    a.v := 99\n    it.v\n  >\n}\n^ = () -> Num => <\n  t = T { v = 1 }\n  t.sneak()\n>",
    );
    assert!(
        error.contains("receiver 'it'"),
        "expected the receiver alias to be rejected, got: {error}"
    );
}

#[test]
fn run_a_mutable_alias_of_the_receiver_inside_a_setter_stays_legal() {
    // A setter's receiver is mutable at every call site, so aliasing it mutably inside
    // the setter is sound — and the write lands on the receiver.
    assert_exit(
        "T = {\n  v :: Num\n  bump := () -> $ => <\n    a := it\n    a.v := 42\n    $\n  >\n}\n^ = () -> Num => <\n  m := T { v = 1 }\n  m.bump()\n  m.v\n>",
        42,
    );
}

// --- Route 5: a field write rooted at a call. ---

#[test]
fn a_field_write_through_a_call_result_aliasing_an_immutable_argument_is_rejected() {
    // Finding 42's shape: `id(t)` IS `t`, so the write is a write to the frozen value.
    let error = type_error_message(
        "T = { v :: Num }\nid = (p :: T) -> T => < p >\n^ = () -> Num => <\n  t = T { v = 1 }\n  id(t).v := 5\n  t.v\n>",
    );
    assert!(
        error.contains("`t`, which is bound with `=`"),
        "expected the call-rooted write to name 't', got: {error}"
    );
}

#[test]
fn a_setter_call_through_a_call_result_aliasing_an_immutable_argument_is_rejected() {
    let error = type_error_message(
        "T = {\n  v :: Num\n  bump := () -> $ => < it.v := 99 >\n}\nid = (p :: T) -> T => < p >\n^ = () -> Num => <\n  t = T { v = 1 }\n  id(t).bump()\n  t.v\n>",
    );
    assert!(
        error.contains("`t`, which is bound with `=`"),
        "expected the call-rooted setter call to name 't', got: {error}"
    );
}

// --- Route 6: plain functions cannot launder a parameter. ---

#[test]
fn a_mutable_alias_of_a_function_parameter_is_rejected() {
    // A parameter's argument belongs to the caller and may be `=`-bound there.
    let error = type_error_message(
        "T = { v :: Num }\nlaunder = (p :: T) -> Num => <\n  a := p\n  a.v := 99\n  p.v\n>\n^ = () -> Num => <\n  t = T { v = 1 }\n  launder(t)\n>",
    );
    assert!(
        error.contains("parameter 'p'"),
        "expected the parameter alias to name 'p', got: {error}"
    );
}

#[test]
fn a_returned_parameter_inherits_the_arguments_mutability_at_the_call_site() {
    // `id` returns its parameter, so `id(t)` IS `t` — binding it `:=` is the alias
    // binding again, however many calls it went through.
    let error = type_error_message(
        "T = { v :: Num }\nid = (p :: T) -> T => < p >\n^ = () -> Num => <\n  t = T { v = 1 }\n  w := id(t)\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the returned parameter to inherit 't''s immutability, got: {error}"
    );
}

#[test]
fn a_parameter_returned_through_a_local_still_inherits_mutability() {
    // Laundering through an intermediate `=` local changes nothing: the local dies at
    // the return, the parameter's value does not.
    let error = type_error_message(
        "T = { v :: Num }\nid = (p :: T) -> T => <\n  x = p\n  x\n>\n^ = () -> Num => <\n  t = T { v = 1 }\n  w := id(t)\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the locally-laundered parameter to inherit 't''s immutability, got: {error}"
    );
}

#[test]
fn run_a_returned_parameter_stays_mutable_for_a_mutable_argument() {
    // The same function on a `:=` argument: the result inherits mutability, and the
    // write reaches the original.
    assert_exit(
        "T = { v :: Num }\nid = (p :: T) -> T => < p >\n^ = () -> Num => <\n  m := T { v = 1 }\n  w := id(m)\n  w.v := 42\n  m.v\n>",
        42,
    );
}

#[test]
fn run_a_function_building_its_result_locally_returns_a_fresh_value() {
    // A local — even a `:=` one — dies at the return, so the result is fresh and binds
    // either way at the call site.
    assert_exit(
        "T = { v :: Num }\nbuild = (start :: Num) -> T => <\n  draft := T { v = start }\n  draft.v := draft.v + 1\n  draft\n>\n^ = () -> Num => <\n  frozen = build(20)\n  open := build(20)\n  open.v := open.v + 0\n  frozen.v + open.v\n>",
        42,
    );
}

#[test]
fn run_passing_a_frozen_record_to_a_function_stays_legal_and_leaves_it_unchanged() {
    // Argument passing is not a binding: a function may read a frozen record freely —
    // and cannot change it.
    assert_exit(
        "T = { v :: Num }\ndouble = (p :: T) -> Num => < p.v * 2 >\n^ = () -> Num => <\n  t = T { v = 20 }\n  double(t) + (t.v == 20 ? 2 : 0)\n>",
        42,
    );
}

#[test]
fn a_lambda_parameter_cannot_be_aliased_mutably_either() {
    // The same parameter rule holds for lambda parameters — including one named `it`,
    // which is an ordinary identifier, not the method receiver.
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  ts = [T { v = 1 }]\n  ts.each(x => <\n    a := x\n    a.v := 9\n    $\n  >)\n  1\n>",
    );
    assert!(
        error.contains("parameter 'x'"),
        "expected the lambda-parameter alias to name 'x', got: {error}"
    );
}

#[test]
fn reassigning_a_mutable_binding_to_a_frozen_value_is_rejected() {
    // The gate also covers REassignment: `m := t` on an existing `:=` binding is the
    // same aliasing route.
    let error = type_error_message(
        "T = { v :: Num }\n^ = () -> Num => <\n  t = T { v = 1 }\n  m := T { v = 2 }\n  m := t\n  t.v\n>",
    );
    assert!(
        error.contains("'t' is immutable"),
        "expected the reassignment to name 't', got: {error}"
    );
}

// --- Route 7: a store into an existing container, in both directions. ---

#[test]
fn a_field_write_storing_an_immutable_record_into_a_mutable_container_is_rejected() {
    // `b.item := c` reaches `c`'s frozen value through every later `b.item := ` write —
    // the same crossing `Box { item = c }` in a `:=` binding is rejected for, checked at
    // the store instead of the binding.
    let error = type_error_message(
        "T = { value :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  c = T { value = 30 }\n  b := Box { item = T { value = 1 } }\n  b.item := c\n  c.value\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the field-write store to name 'c' under QN341, got: {error}"
    );
}

#[test]
fn a_setter_storing_an_immutable_argument_into_the_receiver_is_rejected() {
    // `put` stores its parameter into `it.item`; the receiver `b` is `:=`-bound, so
    // `b.put(c)` is the same store as the direct field write, one call deeper.
    let error = type_error_message(
        "T = { value :: Num }\nBox = { item :: T, put := (k :: T) => < it.item := k > }\n^ = () -> Num => <\n  c = T { value = 30 }\n  b := Box { item = T { value = 1 } }\n  b.put(c)\n  c.value\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the setter-argument store to name 'c' under QN341, got: {error}"
    );
}

#[test]
fn run_a_field_write_storing_a_fresh_value_stays_legal() {
    assert_exit(
        "T = { value :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  b := Box { item = T { value = 1 } }\n  b.item := T { value = 5 }\n  b.item.value\n>",
        5,
    );
}

#[test]
fn run_a_setter_storing_a_fresh_argument_stays_legal() {
    assert_exit(
        "T = { value :: Num }\nBox = { item :: T, put := (k :: T) => < it.item := k > }\n^ = () -> Num => <\n  b := Box { item = T { value = 1 } }\n  b.put(T { value = 5 })\n  b.item.value\n>",
        5,
    );
}

#[test]
fn run_a_setter_reading_only_a_scalar_field_of_its_parameter_accepts_an_immutable_argument() {
    // `put` never stores `k` ITSELF into `it` — only `k.value`, a `Num` copy — so the
    // store rule has nothing to say about `k`'s own binding: an `=`-bound argument is
    // legal, precisely because `setter_stored_parameter_slots` tracks only the
    // parameters a setter's body actually stores by reference.
    assert_exit(
        "T = { value :: Num }\nBox = { item :: T, put := (k :: T) => < it.item.value := k.value > }\n^ = () -> Num => <\n  k = T { value = 9 }\n  b := Box { item = T { value = 1 } }\n  b.put(k)\n  b.item.value\n>",
        9,
    );
}

// --- Route 8: lambdas, higher-order calls, and closures returning a capture. ---

#[test]
fn a_map_callback_returning_a_captured_immutable_value_is_rejected() {
    // The callback ignores its element and always returns `c`: `map`'s result carries
    // `c`'s aliasing exactly as a named function returning its captured local would.
    let error = type_error_message(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  arr := [T { value = 0 }]\n  arr := arr.map(k => c)\n  c.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the map-callback result to name 'c', got: {error}"
    );
}

#[test]
fn a_reduce_callback_returning_a_captured_immutable_value_is_rejected() {
    let error = type_error_message(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  x := [1].reduce(T { value = 0 }, (acc, n) => c)\n  c.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the reduce-callback result to name 'c', got: {error}"
    );
}

#[test]
fn an_immediately_invoked_lambda_returning_a_captured_immutable_value_is_rejected() {
    let error = type_error_message(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  x := (() -> T => < c >)()\n  c.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the immediately-invoked lambda's result to name 'c', got: {error}"
    );
}

#[test]
fn a_closure_returned_from_a_function_that_captured_its_own_local_is_rejected() {
    // `mk` returns a closure holding its own `=` local `c`: every call to that closure
    // returns the SAME value, so it is not fresh — `f = mk()` then `x := f()` launders
    // `c` exactly as `x := mk()()` would.
    let error = type_error_message(
        "T = { value :: Num }\nmk = () -> () -> T => <\n  c = T { value = 30 }\n  () -> T => < c >\n>\n^ = () -> Num => <\n  f = mk()\n  x := f()\n  f().value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the returned closure's result to name 'c', got: {error}"
    );
}

#[test]
fn run_a_map_callback_building_a_fresh_value_stays_legal() {
    assert_exit(
        "T = { value :: Num }\n^ = () -> Num => <\n  arr := [T { value = 0 }]\n  arr := arr.map(k => T { value = 1 })\n  arr[0].value\n>",
        1,
    );
}

#[test]
fn run_a_closure_returning_a_fresh_value_binds_mutably() {
    assert_exit(
        "T = { value :: Num }\ncapture = () -> T => < T { value = 7 } >\n^ = () -> Num => <\n  x := capture()\n  x.value\n>",
        7,
    );
}

#[test]
fn run_an_immediately_invoked_lambda_returning_a_fresh_value_binds_mutably() {
    assert_exit(
        "T = { value :: Num }\n^ = () -> Num => <\n  x := (() -> T => < T { value = 9 } >)()\n  x.value\n>",
        9,
    );
}

// --- Route 9: a store reaching `it` through an element read, not just a field chain. ---

#[test]
fn run_a_setter_storing_a_fresh_value_through_an_indexed_receiver_path_stays_legal() {
    // `it.items[i].sub := v` reaches the receiver through an `Index` hop, not a plain
    // field chain — the store's own gate must still see it as reaching `it` (deferred to
    // the call-site check below), rather than rejecting it unconditionally at the
    // setter's definition, which the naive field-path walk did.
    assert_exit(
        "Inner = { n :: Num }\nT = { sub :: Inner }\nBox = { items :: []T, put := (i :: Num, v :: Inner) => < it.items[i].sub := v > }\n^ = () -> Num => <\n  b := Box { items = [T { sub = Inner { n = 1 } }] }\n  b.put(0, Inner { n = 9 })\n  b.items[0].sub.n\n>",
        9,
    );
}

#[test]
fn a_setter_storing_an_immutable_argument_through_an_indexed_receiver_path_is_rejected() {
    let error = type_error_message(
        "Inner = { n :: Num }\nT = { sub :: Inner }\nBox = { items :: []T, put := (i :: Num, v :: Inner) => < it.items[i].sub := v > }\n^ = () -> Num => <\n  c = Inner { n = 30 }\n  b := Box { items = [T { sub = Inner { n = 1 } }] }\n  b.put(0, c)\n  c.n\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the indexed-receiver store to name 'c' under QN341, got: {error}"
    );
}

// --- Route 10: reassigning a `:=` closure binding reclassifies it, not just the first time. ---

#[test]
fn reassigning_a_closure_binding_to_one_returning_a_fresh_value_drops_the_earlier_capture() {
    // `f` first holds a closure returning its captured `=` local `c`; reassigning it to
    // one that builds a fresh value must reclassify `f`, not leave the earlier,
    // non-default classification stale on the same symbol.
    assert_type_checks(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  capture = () -> T => < c >\n  fresh = () -> T => < T { value = 1 } >\n  f := capture\n  f := fresh\n  x := f()\n  x.value\n>",
    );
}

#[test]
fn reassigning_a_closure_binding_to_one_returning_a_capture_is_still_rejected() {
    let error = type_error_message(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  capture = () -> T => < c >\n  fresh = () -> T => < T { value = 1 } >\n  f := fresh\n  f := capture\n  x := f()\n  x.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the reassigned closure's result to name 'c', got: {error}"
    );
}

// --- Route 11: a curried function's returned closure over its OWN parameter. ---

#[test]
fn run_a_curried_functions_returned_closure_over_a_fresh_argument_stays_legal() {
    // `mk` returns a closure over its OWN parameter `v`, not a captured local — calling
    // `mk(x)()` must inherit `x`'s mutability at the call site, the same way a directly
    // returned parameter already does, rather than treating `v` as a permanent witness.
    assert_type_checks(
        "T = { value :: Num }\nmk = (v :: T) -> () -> T => <\n  () -> T => < v >\n>\n^ = () -> Num => <\n  x := T { value = 5 }\n  y := mk(x)()\n  y.value\n>",
    );
}

#[test]
fn a_curried_functions_returned_closure_over_an_immutable_argument_is_rejected() {
    let error = type_error_message(
        "T = { value :: Num }\nmk = (v :: T) -> () -> T => <\n  () -> T => < v >\n>\n^ = () -> Num => <\n  x = T { value = 5 }\n  y := mk(x)()\n  y.value\n>",
    );
    assert!(
        error.contains("'x' is immutable"),
        "expected the curried closure's result to name 'x', got: {error}"
    );
}

// --- Route 12: a closure chosen between branches. ---

#[test]
fn a_branch_selected_closure_that_may_return_a_capture_is_rejected() {
    // `f` may hold either closure depending on `cond`; a call through it must be
    // rejected whenever EITHER branch could return a captured `=` local, the same way
    // `value_aliasing`'s own `If` handling merges both branches for a reference-typed
    // result.
    let error = type_error_message(
        "T = { value :: Num }\n^ = () -> Num => <\n  c = T { value = 30 }\n  capture = () -> T => < c >\n  fresh = () -> T => < T { value = 1 } >\n  cond = 1 == 1\n  f := cond ? capture : fresh\n  x := f()\n  x.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the branch-selected closure's result to name 'c', got: {error}"
    );
}

// --- Route 13: a setter forwarding its own parameter to a NESTED setter on `it`. ---

#[test]
fn run_a_setter_forwarding_a_fresh_argument_to_a_nested_setter_stays_legal() {
    // `put` forwards its parameter `k` to `it.inner.set(k)`, a setter that itself
    // stores its parameter into `it`. `k` is `put`'s own parameter — unknown at `put`'s
    // definition — so the store check defers to `put`'s own callers instead of
    // rejecting it unconditionally, the same way a direct `it.field := k` write would.
    assert_exit(
        "Counter = { value :: Num }\nInner = { item :: Counter, set := (k :: Counter) => < it.item := k > }\nBox = { inner :: Inner, put := (k :: Counter) => < it.inner.set(k) > }\n^ = () -> Num => <\n  b := Box { inner = Inner { item = Counter { value = 1 } } }\n  b.put(Counter { value = 5 })\n  b.inner.item.value\n>",
        5,
    );
}

#[test]
fn a_setter_forwarding_an_immutable_argument_to_a_nested_setter_is_rejected() {
    // The deferred check lands on `put`'s OWN callers: passing an `=`-bound value into
    // `put` reaches `it.item` two calls deep, exactly as if `put` had written it there
    // directly.
    let error = type_error_message(
        "Counter = { value :: Num }\nInner = { item :: Counter, set := (k :: Counter) => < it.item := k > }\nBox = { inner :: Inner, put := (k :: Counter) => < it.inner.set(k) > }\n^ = () -> Num => <\n  c = Counter { value = 30 }\n  b := Box { inner = Inner { item = Counter { value = 1 } } }\n  b.put(c)\n  b.inner.item.value := 9\n  c.value\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the forwarded-argument store to name 'c' under QN341, got: {error}"
    );
}

// --- Route 14: `it` is an ordinary identifier — a `:=` local named `it` is not the receiver. ---

#[test]
fn a_field_write_through_a_plain_local_named_it_is_still_checked() {
    // `it` is not a keyword: a `:=` local outside any method that happens to be named
    // `it` is an ordinary mutable binding, not a setter's receiver — the store check
    // must not defer to it as though it were.
    let error = type_error_message(
        "T = { value :: Num }\nBox = { item :: T }\n^ = () -> Num => <\n  c = T { value = 30 }\n  it := Box { item = T { value = 1 } }\n  it.item := c\n  c.value\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the plain-local field write to name 'c' under QN341, got: {error}"
    );
}

#[test]
fn a_setter_call_through_a_plain_local_named_it_is_still_checked() {
    let error = type_error_message(
        "T = { value :: Num }\nBox = { item :: T, put := (k :: T) => < it.item := k > }\n^ = () -> Num => <\n  c = T { value = 30 }\n  it := Box { item = T { value = 1 } }\n  it.put(c)\n  c.value\n>",
    );
    assert!(
        error.contains("QN341") && error.contains("'c' is immutable"),
        "expected the plain-local setter call to name 'c' under QN341, got: {error}"
    );
}

// --- Route 15: a closure three levels deep, returning a captured value. ---

#[test]
fn a_closure_three_levels_deep_returning_a_captured_value_is_rejected() {
    // `mk`'s own body is a lambda returning another lambda, which in turn returns the
    // captured local `c` — one syntactic level past the two-level case already covered
    // above, classified through the same rule at each level.
    let error = type_error_message(
        "T = { value :: Num }\nmk = () -> () -> () -> T => <\n  c = T { value = 30 }\n  () -> () -> T => <\n    () -> T => < c >\n  >\n>\n^ = () -> Num => <\n  x := mk()()()\n  x.value\n>",
    );
    assert!(
        error.contains("'c' is immutable"),
        "expected the three-level closure's result to name 'c', got: {error}"
    );
}

#[test]
fn run_a_closure_three_levels_deep_returning_a_fresh_value_binds_mutably() {
    assert_exit(
        "T = { value :: Num }\nmk = () -> () -> () -> T => <\n  () -> () -> T => <\n    () -> T => < T { value = 9 } >\n  >\n>\n^ = () -> Num => <\n  x := mk()()()\n  x.value\n>",
        9,
    );
}

// --- Route 16: a derived collection shares its ELEMENTS, not its own identity — a fresh
// array/map of scalars binds either way, but one of reference-typed elements stays tied
// to the source, exactly like an alias of the source itself. ---

#[test]
fn an_alias_of_an_immutable_array_is_rejected() {
    let error =
        type_error_message("^ = () -> Num => <\n  nums = [1, 2, 3]\n  ys := nums\n  ys[0]\n>");
    assert!(
        error.contains("'nums' is immutable"),
        "expected the array alias to name 'nums', got: {error}"
    );
}

#[test]
fn run_map_of_scalars_from_an_immutable_array_is_a_fresh_mutable_array() {
    // `nums.map(...)` builds a fresh array of copied `Num`s: writing into it can never
    // reach `nums`, so it binds `:=` even though `nums` is `=`.
    assert_type_checks(
        "^ = () -> Num => <\n  nums = [1, 2, 3]\n  doubled := nums.map(x => x * 2)\n  doubled[0]\n>",
    );
    assert_exit(
        "^ = () -> Num => <\n  nums = [1, 2, 3]\n  doubled := nums.map(x => x * 2)\n  doubled[0] := 10\n  doubled[0] + nums[0]\n>",
        11,
    );
}

#[test]
fn run_filter_of_scalars_from_an_immutable_array_is_a_fresh_mutable_array() {
    assert_type_checks(
        "^ = () -> Num => <\n  nums = [1, 2, 3]\n  evens := nums.filter(x => x > 1)\n  evens[0]\n>",
    );
}

#[test]
fn run_keys_of_an_immutable_map_of_scalars_is_a_fresh_mutable_array() {
    assert_type_checks("^ = () -> Num => <\n  m = [|\"a\" => 1|]\n  ks := m.keys()\n  ks.size\n>");
}

#[test]
fn filter_of_reference_typed_elements_from_an_immutable_array_is_still_rejected() {
    // `points`'s elements are records: `filter` returns a fresh array, but its kept
    // entries are the SAME record values `points` holds, so the result still ties back
    // to `points` exactly as a direct alias would.
    let error = type_error_message(
        "Point = { x :: Num }\n^ = () -> Num => <\n  points = [Point { x = 1 }]\n  firsts := points.filter(p => p.x > 0)\n  firsts[0].x\n>",
    );
    assert!(
        error.contains("'points' is immutable"),
        "expected the filtered record array to still name 'points', got: {error}"
    );
}
