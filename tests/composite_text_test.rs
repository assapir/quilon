// Text (and other non-`Num` values) nested inside composites must round-trip
// through codegen without f64 corruption. Codegen recovers each element/field/
// match-result type from the type-oracle side-table (see `typechecker::TypeTable`
// and `codegen::TypeOracle`) instead of assuming `f64` at READ sites.
//
// These are execution tests: full pipeline (lex -> parse -> typecheck -> codegen ->
// JIT) asserting the real exit code, so a corrupted value would surface as a wrong
// (often garbage) exit status.

// --- Text field inside a record -------------------------------------------------

mod common;
use common::{assert_exit, assert_type_error};

#[test]
fn record_text_field_reads_back_as_text() {
    // `.name` is a `Text` field; reading it then taking `.length` must see a real
    // Text struct, not an f64 reinterpretation. "Quilon" -> 6 graphemes.
    assert_exit(
        r#"
        ^ = () -> Num => <
          user = { name = "Quilon", n = 7 }
          user.name.length
        >
        "#,
        6,
    );
}

#[test]
fn record_mixed_text_and_num_fields() {
    // A record mixing a `Text` field and a `Num` field: both read back correctly,
    // and the numeric field isn't shifted by the Text field's wider layout.
    // "ab".size (2) + 40 = 42.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = { label = "ab", count = 40 }
          r.label.size + r.count
        >
        "#,
        42,
    );
}

// --- Array of Text --------------------------------------------------------------

#[test]
fn array_of_text_indexes_to_text() {
    // `[]Text`: indexing yields a `Text` value (not f64). "cde".size = 3.
    assert_exit(
        r#"
        ^ = () -> Num => <
          words = ["a", "cde"]
          words[1].size
        >
        "#,
        3,
    );
}

#[test]
fn array_of_text_iterated() {
    // Indexing both elements and summing their byte lengths: "ab"(2)+"cdef"(4)=6.
    assert_exit(
        r#"
        ^ = () -> Num => <
          words = ["ab", "cdef"]
          words[0].size + words[1].size
        >
        "#,
        6,
    );
}

// --- Nested arrays --------------------------------------------------------------

#[test]
fn nested_array_double_index() {
    // `[][]Num`: the outer element is itself an array struct; double-indexing must
    // load the inner array struct first, then the Num. grid[1][0] = 3.
    assert_exit(
        r#"
        ^ = () -> Num => <
          grid = [[1, 2], [3, 4]]
          grid[1][0]
        >
        "#,
        3,
    );
}

#[test]
fn nested_array_sum_of_cells() {
    // Several cells from a nested array: 1 + 4 + 6 = 11.
    assert_exit(
        r#"
        ^ = () -> Num => <
          grid = [[1, 2], [3, 4], [5, 6]]
          grid[0][0] + grid[1][1] + grid[2][1]
        >
        "#,
        11,
    );
}

// --- Text as a sum-type payload (Ok/NotOk) --------------------------------------

#[test]
fn result_ok_text_payload_round_trips() {
    // `Ok("...")`: the Text payload survives construction AND the match-arm result
    // alloca/load (no f64 corruption of the match result). "hello".length = 5.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = Ok("hello")
          r ? | Ok(x) => x.length | NotOk(e) => 0
        >
        "#,
        5,
    );
}

#[test]
fn result_notok_text_payload_round_trips() {
    // `NotOk("...")` with a Text payload. "boom!".size = 5.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = NotOk("boom!")
          r ? | Ok(x) => 0 | NotOk(e) => e.size
        >
        "#,
        5,
    );
}

#[test]
fn user_sum_type_text_payload_round_trips() {
    // A user-defined sum type with `Text` payloads in both variants; matching binds
    // the payload at its real type. "hi there".length = 8.
    assert_exit(
        r#"
        Msg = Hello(Text) / Bye(Text)
        ^ = () -> Num => <
          m = Hello("hi there")
          m ? | Hello(t) => t.length | Bye(t) => t.length
        >
        "#,
        8,
    );
}

#[test]
fn match_result_type_from_unconstructed_generic_arm_compiles() {
    // Regression: the match's result type is taken from the FIRST arm (`NotOk(e) => e`),
    // whose payload `e` stays an un-specialized `Generic` (NotOk is never constructed
    // here). The oracle records the match result as `Generic`; codegen must fall back to
    // the numeric (f64) representation rather than erroring on an unlowerable type.
    // Ok(7) -> the Ok arm runs -> 7.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = Ok(7)
          r ? | NotOk(e) => e | Ok(x) => x
        >
        "#,
        7,
    );
}

#[test]
fn named_constructor_fields_out_of_declaration_order() {
    // Regression: a named-type constructor may list fields in any order; the lowered
    // struct slots must follow DECLARATION order (what field reads GEP against). With a
    // mixed Text+Num record and the call order reversed, a wrong slot order would read
    // the Text field as a Num (or vice versa). "ab".size (2) + 40 = 42.
    assert_exit(
        r#"
        User = {
          name :: Text,
          age :: Num
        }
        ^ = () -> Num => <
          u = User { age = 40, name = "ab" }
          u.name.size + u.age
        >
        "#,
        42,
    );
}

#[test]
fn match_returning_text_then_measured() {
    // The match itself yields `Text` (both arms return a Text), measured afterward.
    // Picks "longer" (6 graphemes).
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = Ok("longer")
          s = r ? | Ok(x) => x | NotOk(e) => e
          s.length
        >
        "#,
        6,
    );
}

// --- Concrete Result payload typing (task #34) ----------------------------------
//
// A pattern-bound `Ok`/`NotOk` payload must carry its CONCRETE type so it is usable at
// the match site — not just readable via `.size`/`.length` (already covered above) but
// also as an overloaded-call argument and across function boundaries. These assert the
// bind-and-USE behavior for every payload kind and the three flows that previously
// failed (overload misdispatch, inferred-return, generic `-> Result` annotation).

#[test]
fn ok_text_payload_dispatches_overload_by_concrete_type() {
    // The bound `Ok` payload `s : Text` must dispatch the overload set to its TEXT
    // member (the old generic-Result behavior wrongly picked the Num member and
    // miscompiled). "quilon".size = 6, via the Text member.
    assert_exit(
        r#"
        describe = (s :: Text) -> Num => < s.size >
        describe = (n :: Num)  -> Num => < n + 100 >
        ^ = () -> Num => <
          r = Ok("quilon")
          r ? | Ok(s) => describe(s) | NotOk(_) => 0
        >
        "#,
        6,
    );
}

#[test]
fn ok_num_payload_dispatches_overload_by_concrete_type() {
    // The numeric payload picks the Num member: 5 + 100 = 105.
    assert_exit(
        r#"
        describe = (s :: Text) -> Num => < s.size >
        describe = (n :: Num)  -> Num => < n + 100 >
        ^ = () -> Num => <
          r = Ok(5)
          r ? | Ok(n) => describe(n) | NotOk(_) => 0
        >
        "#,
        105,
    );
}

#[test]
fn ok_unit_payload_still_matches() {
    // `Ok($)` (unit payload) carries no value; matching `Ok(_)` still works.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = Ok($)
          r ? | Ok(_) => 7 | NotOk(_) => 0
        >
        "#,
        7,
    );
}

#[test]
fn ok_bool_payload_binds_and_is_usable() {
    // A `Bool` payload binds at `Bool` and drives a ternary. Ok(true) -> 1.
    assert_exit(
        r#"
        ^ = () -> Num => <
          r = Ok(true)
          r ? | Ok(b) => (b ? 1 : 2) | NotOk(_) => 0
        >
        "#,
        1,
    );
}

#[test]
fn notok_text_payload_dispatches_overload() {
    // The error payload is Text too and dispatches by its concrete type. "oops".size = 4.
    assert_exit(
        r#"
        len = (s :: Text) -> Num => < s.size >
        len = (n :: Num)  -> Num => < n >
        ^ = () -> Num => <
          r = NotOk("oops")
          r ? | Ok(_) => 0 | NotOk(e) => len(e)
        >
        "#,
        4,
    );
}

#[test]
fn inferred_return_result_payload_is_usable() {
    // Case C: an UNANNOTATED function returning `Ok("...")`. Codegen must lower its
    // return to the payload's real shape (not the historical `Num` default) so a
    // downstream match binds the Text payload usably. "hello".size = 5.
    assert_exit(
        r#"
        make = () => < Ok("hello") >
        ^ = () -> Num => <
          r = make()
          r ? | Ok(s) => s.size | NotOk(_) => 0
        >
        "#,
        5,
    );
}

#[test]
fn annotated_result_return_carries_concrete_payload() {
    // Case A: a `-> Result` annotated function whose body pins `Ok(Text)`. The generic
    // annotation is refined to the concrete body type, so the caller's `Ok(s)` binds
    // `s : Text`. "world".size = 5.
    assert_exit(
        r#"
        make = () -> Result => < Ok("world") >
        ^ = () -> Num => <
          r = make()
          r ? | Ok(s) => s.size | NotOk(_) => 0
        >
        "#,
        5,
    );
}

#[test]
fn getenv_shaped_result_both_arms_text() {
    // The `getEnv`/`getOpt` shape: `-> Result` returning `Ok(Text)` in one branch and
    // `NotOk(Text)` in the other. Both branch payloads must survive to the caller so
    // EITHER arm can use its Text. Looking up a missing key -> NotOk("unset").size = 5.
    assert_exit(
        r#"
        lookup = (key :: Text) -> Result => <
          key == "home"
            ? Ok("/usr/home")
            : NotOk("unset")
        >
        ^ = () -> Num => <
          lookup("nope") ?
            | Ok(path)   => path.size
            | NotOk(err) => err.size
        >
        "#,
        5,
    );
}

#[test]
fn getenv_shaped_result_ok_branch_uses_text() {
    // Same helper, the Ok branch: "/usr/home".size = 9.
    assert_exit(
        r#"
        lookup = (key :: Text) -> Result => <
          key == "home"
            ? Ok("/usr/home")
            : NotOk("unset")
        >
        ^ = () -> Num => <
          lookup("home") ?
            | Ok(path)   => path.size
            | NotOk(err) => err.size
        >
        "#,
        9,
    );
}

#[test]
fn heterogeneous_result_payload_across_branches_still_compiles() {
    // Regression: a `Result` whose payload TYPE differs across branches — `Ok($)` (unit)
    // vs `NotOk(Num)`. The unit payload shares the canonical numeric slot, so both
    // branches keep one struct shape. check(142) -> NotOk(142) -> 142.
    assert_exit(
        r#"
        check = (n :: Num) -> Result => < n <= 100 ? Ok($) : NotOk(n) >
        ^ = () -> Num => < check(142) ? | Ok(_) => 0 | NotOk(c) => c >
        "#,
        142,
    );
}

#[test]
fn overload_with_no_matching_member_for_payload_is_rejected() {
    // Negative: the bound payload is `Text`, but no overload member takes `Text`.
    // With concrete payload typing this is a clean "no matching overload" error, not a
    // silent fallback to the Num member (the old generic-Result behavior).
    assert_type_error(
        r#"
        only = (n :: Num)  -> Num => < n >
        only = (b :: Bool) -> Num => < 1 >
        ^ = () -> Num => <
          r = Ok("x")
          r ? | Ok(s) => only(s) | NotOk(_) => 0
        >
        "#,
    );
}
