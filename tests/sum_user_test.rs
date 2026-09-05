//! End-to-end tests for user-defined sum types (the `/` separator):
//! declaration, construction, and exhaustive pattern matching that extracts
//! payloads. Drives the full pipeline (lex -> parse -> typecheck -> codegen ->
//! JIT) and asserts the entry point's real exit code, plus negative
//! type-checking cases (non-exhaustive match, bad payload type, duplicate
//! variant names). Result is exercised here too, as a *normal* predefined sum
//! type, to prove the general mechanism subsumes the old special case.

mod common;
use common::{assert_exit, assert_type_error, assert_type_error_code};
use quilon::diagnostic::codes::Code;

#[test]
fn nullary_enum_matched_exhaustively() {
    // A nullary enum `Color`, constructed and matched on every variant.
    // Green is the second variant (tag 1); the match maps it to exit code 1.
    assert_exit(
        "Color = Red / Green / Blue\n\
         ^ = () -> Num => <\n\
           c = Green\n\
           c ?\n\
             | Red => 0\n\
             | Green => 1\n\
             | Blue => 2\n\
         >",
        1,
    );
}

#[test]
fn payload_sum_constructed_and_matched() {
    // `Shape` with payload variants; match extracts the payloads. A Rect(3, 4)
    // contributes 3 + 4 = 7. (`Circle` arm is also covered for exhaustiveness.)
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         ^ = () -> Num => <\n\
           s = Rect(3, 4)\n\
           s ?\n\
             | Circle(r) => r\n\
             | Rect(w, h) => w + h\n\
         >",
        7,
    );
}

#[test]
fn payload_sum_single_field_extracted() {
    // Single-payload variant: Circle(9) -> 9.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         ^ = () -> Num => <\n\
           s = Circle(9)\n\
           s ?\n\
             | Circle(r) => r\n\
             | Rect(w, h) => w + h\n\
         >",
        9,
    );
}

#[test]
fn function_over_sum_type_param_and_match() {
    // A function takes a sum-type parameter (`s :: Shape`) and dispatches on it —
    // exercises lowering a sum-type annotation to its tagged-union struct and passing
    // a constructed value as an argument. area(Rect(6, 7)) = 42.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
           area = (s :: Shape) -> Num => <\n\
             s ?\n\
             | Circle(r)  => 3 * r * r\n\
             | Rect(w, h) => w * h\n\
           >\n\
           ^ = () -> Num => < area(Rect(6, 7)) >",
        42,
    );
}

#[test]
fn result_still_works_as_a_normal_sum_type() {
    // The predefined Result behaves exactly as before, now via the general
    // sum-type mechanism: Ok(42) matched, payload doubled -> 84.
    assert_exit(
        "^ = () -> Num => <\n\
           outcome = Ok(42)\n\
           outcome ?\n\
             | Ok(x) => x * 2\n\
             | NotOk(e) => 0\n\
         >",
        84,
    );
}

#[test]
fn result_with_unit_payload_ok_dollar() {
    // `Ok($)` is the canonical "succeeded, no meaningful value" Result (like
    // `Result<(), E>`). A function returns `Ok($)` on success / `NotOk(code)` on
    // failure; matching `Ok(_)` (ignoring the unit payload) yields 0, `NotOk(c)`
    // yields the code. Here the success branch is taken -> exit 0.
    assert_exit(
        "validate = (n :: Num) -> Result => < n <= 10 ? Ok($) : NotOk(n) >\n\
           ^ = () -> Num => <\n\
             validate(5) ?\n\
             | Ok(_)     => 0\n\
             | NotOk(c)  => c\n\
           >",
        0,
    );
}

#[test]
fn result_with_unit_payload_notok_path() {
    // Same shape, failure branch: validate(20) -> NotOk(20) -> exit 20.
    assert_exit(
        "validate = (n :: Num) -> Result => < n <= 10 ? Ok($) : NotOk(n) >\n\
           ^ = () -> Num => <\n\
             validate(20) ?\n\
             | Ok(_)     => 0\n\
             | NotOk(c)  => c\n\
           >",
        20,
    );
}

#[test]
fn user_sum_with_unit_payload() {
    // `$` is a valid payload for a user sum type too, and may coexist with a
    // concrete-typed field at the same position (`Done($)` vs `Pending(Num)`).
    assert_exit(
        "Job = Done($) / Pending(Num)\n\
         ^ = () -> Num => <\n\
           j = Pending(7)\n\
           j ?\n\
             | Done(_)    => 0\n\
             | Pending(n) => n\n\
         >",
        7,
    );
}

#[test]
fn non_exhaustive_match_is_rejected() {
    // Missing the `Blue` arm (and no wildcard) over a user sum type must not compile.
    assert_type_error(
        "Color = Red / Green / Blue\n\
           classify = (c :: Color) -> Num => <\n\
             c ?\n\
             | Red => 0\n\
             | Green => 1\n\
           >",
    );
}

#[test]
fn non_builtin_payload_type_is_rejected() {
    // Payloads are built-in types only (Num / Text / Bool). A user type as a
    // payload (here the sum type referencing itself) is rejected.
    assert_type_error("Tree = Leaf / Node(Tree)");
}

#[test]
fn heterogeneous_payload_position_is_rejected() {
    // A sum type's payload slot has one shared representation per position, so two
    // variants disagreeing on a concrete type at the same position (Num vs Text)
    // would miscompile — the checker rejects it instead. (`$` may still coexist with
    // a concrete type; that's covered by `user_sum_with_unit_payload`.)
    assert_type_error("Mixed = A(Num) / B(Text)");
}

#[test]
fn duplicate_variant_names_are_rejected() {
    // Variant (constructor) names must be unique per scope — `Red` twice fails.
    assert_type_error("A = Red / Green\nB = Red / Blue");
}

#[test]
fn num_redeclared_as_a_sum_type_is_rejected() {
    // A built-in type's name is reserved: a type declared under it is refused as such,
    // not as a duplicate of some binding.
    assert_type_error_code("Num = Foo / Bar", Code::ReservedName);
}

#[test]
fn bool_redeclared_as_a_sum_type_is_rejected() {
    assert_type_error_code("Bool = Yes / No", Code::ReservedName);
}

#[test]
fn text_redeclared_as_a_record_type_is_rejected() {
    assert_type_error_code("Text = { x :: Num }", Code::ReservedName);
}

#[test]
fn result_and_site_redeclared_are_reserved_not_duplicates() {
    assert_type_error_code("Result = A / B", Code::ReservedName);
    assert_type_error_code("Site = A / B", Code::ReservedName);
}

#[test]
fn literal_payload_pattern_is_rejected() {
    // Codegen dispatches on the constructor TAG alone, so `Ok(1)` would silently
    // match ANY `Ok` payload (the wrong arm wins with no diagnostic). Until payload
    // tests are implemented, a refutable sub-pattern is a type error.
    assert_type_error(
        "^ = () -> Num => <\n  r = Ok(2)\n  r ?\n    | Ok(1) => 10\n    | NotOk(e) => 20\n>",
    );
}

#[test]
fn nested_constructor_payload_pattern_is_rejected() {
    // A nested constructor sub-pattern is refutable too.
    assert_type_error(
        "^ = () -> Num => <\n  r = Ok(2)\n  r ?\n    | Ok(Ok(x)) => 10\n    | _ => 20\n>",
    );
}

#[test]
fn literal_payload_pattern_in_user_sum_is_rejected() {
    // Same rule for USER sum types, not just the built-in Result.
    assert_type_error(
        "Shape = Circle(Num) / Square(Num)\n^ = () -> Num => <\n  s = Circle(3)\n  s ?\n    | Circle(3) => 1\n    | Square(n) => 2\n>",
    );
}

#[test]
fn binding_and_wildcard_payload_patterns_still_accepted() {
    // The irrefutable forms — a binding and `_` — keep working.
    assert_exit(
        "^ = () -> Num => <\n  r = Ok(2)\n  r ?\n    | Ok(x) => x + 1\n    | NotOk(_) => 20\n>",
        3,
    );
}

#[test]
fn record_field_typed_as_user_sum() {
    // A record field annotated as a user SUM type carries the sum: construct the
    // record with a variant value, read the field back, and dispatch on it. `verb`
    // maps Post to 2, so exit 2 proves the field really dispatched.
    assert_exit(
        "Method = Get / Post\n\
         Request = { method :: Method, tag :: Num }\n\
         verb = (method :: Method) -> Num => < method ? | Get => 1 | Post => 2 >\n\
         ^ = () -> Num => <\n\
           request = Request { method = Post, tag = 9 }\n\
           verb(request.method)\n\
         >",
        2,
    );
}

#[test]
fn bound_result_with_record_payload_matched() {
    // A `Result` carrying a RECORD payload, bound to a `:: Result` variable and then
    // matched extracting the record — the binding must keep the concrete payload type
    // so the match unpacks the record (a pointer), not the numeric fallback.
    // Point { x = 3, y = 4 } -> x + y = 7.
    assert_exit(
        "Point = { x :: Num, y :: Num }\n\
         wrap = (point :: Point) -> Result => < Ok(point) >\n\
         ^ = () -> Num => <\n\
           boxed :: Result = wrap(Point { x = 3, y = 4 })\n\
           got :: Point = boxed ?\n\
             | Ok(inner) => inner\n\
             | NotOk(_)  => Point { x = 0, y = 0 }\n\
           got.x + got.y\n\
         >",
        7,
    );
}

#[test]
fn method_returning_result_keeps_a_text_payload() {
    // A METHOD annotated `-> Result` returning `Ok(text)`: the generic annotation must be
    // refined to the inferred body type, exactly as a top-level function's is, or the match
    // below binds the payload at the numeric fallback and reads the Text back as garbage.
    // `.length` of "hello" is 5.
    assert_exit(
        "Box = {\n\
           tag :: Num,\n\
           pick = () -> Result => < Ok(\"hello\") >\n\
         }\n\
         ^ = () -> Num => <\n\
           box = Box { tag = 1 }\n\
           box.pick() ?\n\
             | Ok(value) => value.length\n\
             | NotOk(_)  => 0\n\
         >",
        5,
    );
}

#[test]
fn method_returning_result_keeps_a_record_payload() {
    // The same refinement for an aggregate payload: a method's `-> Result` carrying a
    // RECORD must unpack as that record and not as the numeric fallback (which segfaulted).
    // Point { x = 3, y = 4 } -> x + y = 7.
    assert_exit(
        "Point = { x :: Num, y :: Num }\n\
         Maker = {\n\
           seed :: Num,\n\
           build = () -> Result => < Ok(Point { x = 3, y = 4 }) >\n\
         }\n\
         ^ = () -> Num => <\n\
           maker = Maker { seed = 0 }\n\
           got = maker.build() ?\n\
             | Ok(inner) => inner\n\
             | NotOk(_)  => Point { x = 0, y = 0 }\n\
           got.x + got.y\n\
         >",
        7,
    );
}
