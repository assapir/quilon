// Text (and other non-`Num` values) nested inside composites must round-trip
// through codegen without f64 corruption. Codegen recovers each element/field/
// match-result type from the type-oracle side-table (see `typechecker::TypeTable`
// and `codegen::TypeOracle`) instead of assuming `f64` at READ sites.
//
// These are execution tests: full pipeline (lex -> parse -> typecheck -> codegen ->
// JIT) asserting the real exit code, so a corrupted value would surface as a wrong
// (often garbage) exit status.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / target init isn't thread-safe; serialize across cargo's parallel tests.
static JIT_LOCK: Mutex<()> = Mutex::new(());

fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code = jit::run_program(&program).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{src}");
}

// --- Text field inside a record -------------------------------------------------

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
