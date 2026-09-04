// Execution-based tests: drive the full pipeline (lex -> parse -> typecheck ->
// codegen -> JIT) and assert the program's real exit code. This is the backbone
// that makes documented example behavior ("factorial(5) -> 120") actually verified.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::path::Path;

mod common;
use common::{
    JIT_LOCK, assert_exit, assert_exit_linked, assert_exit_linked_from, assert_type_error,
    build_and_run_native, tool_available,
};

#[test]
fn run_simple_arithmetic() {
    // examples/simple.qn
    assert_exit("^ = () -> Num => <\n  a = 5\n  b = 7\n  a + b\n>", 12);
}

#[test]
fn run_subtraction_and_multiplication() {
    // Explicit end-to-end coverage for the `-` and `*` codegen arms. 1 + 2 = 3.
    assert_exit(
        "^ = () -> Num => < (50 - 8 == 42 ? 1 : 0) + (6 * 7 == 42 ? 2 : 0) >",
        3,
    );
}

#[test]
fn run_unary_minus_and_double_negation() {
    // `-x` negates; `-(-x)` round-trips back to the original value. 1 + 2 = 3.
    assert_exit(
        "^ = () -> Num => <\n  x = 5\n  y = (-x == -5 ? 1 : 0) + (-(-x) == 5 ? 2 : 0)\n  y\n>",
        3,
    );
}

#[test]
fn run_division_and_mixed_fractional_arithmetic() {
    // `/` produces fractions that keep working in further arithmetic, and
    // integer/fractional literals mix freely. 1 + 2 + 4 + 8 = 15.
    assert_exit(
        "^ = () -> Num => < (7 / 2 == 3.5 ? 1 : 0) + (7 / 2 + 7 / 2 == 7 ? 2 : 0) + (42 + 3.14 == 45.14 ? 4 : 0) + (2.5 * 4 == 10 ? 8 : 0) >",
        15,
    );
}

#[test]
fn run_arithmetic_precedence_and_parentheses() {
    // `*` / `/` / `%` bind tighter than `+` / `-`; parentheses override. 1 + 2 + 4 + 8 = 15.
    assert_exit(
        "^ = () -> Num => < (2 + 3 * 4 == 14 ? 1 : 0) + ((2 + 3) * 4 == 20 ? 2 : 0) + (10 - 6 / 2 == 7 ? 4 : 0) + (20 % 7 - 2 * 3 == 0 ? 8 : 0) >",
        15,
    );
}

#[test]
fn run_factorial() {
    // examples/factorial.qn -> factorial(5) = 120
    assert_exit(
        "factorial = (n :: Num) -> Num => < n <= 1 ? 1 : n * factorial(n - 1) >\n\n^ = () -> Num => < factorial(5) >",
        120,
    );
}

#[test]
fn run_fibonacci() {
    // examples/fibonacci.qn -> fib(10) = 55
    assert_exit(
        "fib = (n :: Num) -> Num => < n <= 1 ? n : fib(n - 1) + fib(n - 2) >\n\n^ = () -> Num => < fib(10) >",
        55,
    );
}

#[test]
fn run_array_size() {
    // examples/array_size.qn -> [1,2,3,4,5].size = 5
    assert_exit(
        "^ = () -> Num => <\n  nums = [1, 2, 3, 4, 5]\n  nums.size\n>",
        5,
    );
}

#[test]
fn run_pattern_match_number() {
    // examples/option.qn -> matches the `5` arm
    assert_exit(
        "^ = () -> Num => <\n  value = 5\n  result = value ?\n    | 5 => 50\n    | 3 => 30\n    | _ => 0\n  result\n>",
        50,
    );
}

#[test]
fn run_pattern_match_wildcard() {
    // examples/pattern_wildcard.qn -> falls through to `_`
    assert_exit(
        "^ = () -> Num => <\n  value = 7\n  result = value ?\n    | 5 => 50\n    | 3 => 30\n    | _ => 99\n  result\n>",
        99,
    );
}

// --- Text: { ptr, byte_len }, with `+` concatenation, `.size` (bytes) and
//     `.length` (grapheme clusters). "héllo" + " 🌍":
//       bytes     = 6 ("héllo": é is 2 bytes) + 5 (" 🌍": 🌍 is 4 bytes) = 11
//       graphemes = 5 + 2 = 7   (so graphemes < bytes for multibyte/emoji input)

#[test]
fn run_text_concat_byte_size() {
    assert_exit("^ = () -> Num => < (\"héllo\" + \" 🌍\").size >", 11);
}

#[test]
fn run_text_grapheme_length() {
    assert_exit("^ = () -> Num => < (\"héllo\" + \" 🌍\").length >", 7);
}

#[test]
fn run_text_ascii_concat_size() {
    // ASCII: bytes == graphemes.
    assert_exit("^ = () -> Num => <\n  s = \"ab\" + \"cde\"\n  s.size\n>", 5);
}

#[test]
fn run_record_size_field_not_shadowed() {
    // Regression: a record field literally named `size` must resolve by NAME
    // (field 0 here -> 7), not be hijacked by the Text/array `.size` struct-shape
    // handling (which would read field index 1 -> 9).
    assert_exit(
        "^ = () -> Num => <\n  r = { size = 7, other = 9 }\n  r.size\n>",
        7,
    );
}

// --- IO: write / print over `<< core.io` ---

#[test]
fn run_write_to_stdout_returns_byte_count() {
    // write returns bytes written = 2.
    assert_exit_linked(
        "<< core.io\n^ = () -> Num => < io.write(\"hi\", io.stdout) >",
        2,
    );
}

#[test]
fn run_print_text_then_exit() {
    // print writes "hello\n" to stdout and yields Num 0.
    assert_exit_linked(
        "<< core.io\n^ = () -> Num => <\n  io.print(\"hello\")\n  0\n>",
        0,
    );
}

// --- Iteration via the `.each` array method (the `for`-loop replacement) ---

#[test]
fn run_each_executes() {
    // `.each` runs a body for its side effects and returns the receiver; this
    // proves the array-method iteration path parses, type-checks, and runs.
    assert_exit(
        "^ = () -> Num => <\n  xs = [1, 2, 3]\n  xs.each(n => n)\n  0\n>",
        0,
    );
}

#[test]
fn run_each_with_block_body() {
    // The `.each` lambda body may be a `< ... >` block, closing right before the
    // call's `)`.
    assert_exit(
        "^ = () -> Num => <\n  xs = [10, 20, 30]\n  xs.each(val => <\n    x = val + 1\n    x\n  >)\n  0\n>",
        0,
    );
}

// --- Implicit exit-0 for the entry point `^` (C main-style success) ---
// When `^`'s body isn't a Num, the program runs the body for its side effects
// and exits 0, so a side-effecting main needs no trailing `0`. A Num body is
// still used as the exit code. Scoped to `^`; ordinary functions are unaffected.

#[test]
fn run_entry_non_num_body_exits_zero() {
    // Body is a Text value, not a Num -> implicit exit 0.
    assert_exit("^ = () => < \"done\" >", 0);
}

#[test]
fn run_entry_num_body_still_is_exit_code() {
    // A Num body is unchanged: it becomes the exit code.
    assert_exit("^ = () -> Num => < 42 >", 42);
}

#[test]
fn run_entry_side_effecting_main_no_trailing_zero() {
    // `<< core.io` + a print as the last expression, with NO trailing 0 -> exit 0.
    assert_exit_linked("<< core.io\n^ = () => < io.print(\"hi\") >", 0);
}

// --- Mutability: `:=` declares a mutable binding and reassigns it; `=` is immutable. ---

#[test]
fn run_mutable_declare_and_reassign() {
    // Declare with `:=`, reassign with `:=`; the final value is the exit code.
    assert_exit(
        "^ = () -> Num => <\n  counter := 0\n  counter := counter + 5\n  counter := counter + 37\n  counter\n>",
        42,
    );
}

#[test]
fn reassigning_immutable_binding_is_a_type_error() {
    // `x` is immutable (`=`); reassigning it with `:=` must fail type checking.
    let src = "^ = () -> Num => <\n  x = 1\n  x := 2\n  x\n>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected reassigning an immutable binding to be a type error"
    );
}

// --- In-place record mutation: `:=` instances allow field writes + setter methods. ---

#[test]
fn run_mutable_record_field_write_mutates_in_place() {
    // A `:=`-bound record allows a direct in-place field write `c.value := …`.
    // The mutation is observable on the same binding afterwards.
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump := (by :: Num) => < it.value := it.value + by >\n}\n^ = () -> Num => <\n  c := Counter { value = 30 }\n  c.value := c.value + 12\n  c.value\n>",
        42,
    );
}

#[test]
fn run_setter_method_mutates_mutable_instance() {
    // A setter method (declared `:=`) mutates a `:=` instance in place; the change is
    // visible through the same binding after the call.
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump := (by :: Num) => < it.value := it.value + by >\n}\n^ = () -> Num => <\n  c := Counter { value = 30 }\n  c.bump(5)\n  c.bump(7)\n  c.value\n>",
        42,
    );
}

#[test]
fn run_setter_with_block_body_writes_multiple_fields() {
    // A `:=` method whose body is a `< >` block performing several `it.f := …` writes
    // mutates every field in place.
    assert_exit(
        "Point = {\n  x :: Num,\n  y :: Num,\n  shift := (d :: Num) => <\n    it.x := it.x + d\n    it.y := it.y + d\n  >\n}\n^ = () -> Num => <\n  p := Point { x = 1, y = 2 }\n  p.shift(10)\n  p.x + p.y\n>",
        23,
    );
}

#[test]
fn field_write_on_immutable_instance_is_a_type_error() {
    // `c` is bound with `=` (immutable); a direct field write `c.value := …`
    // must fail type checking — immutable instances are frozen.
    let src = "Counter = {\n  value :: Num\n}\n^ = () -> Num => <\n  c = Counter { value = 30 }\n  c.value := 99\n  c.value\n>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected a field write on an `=`-bound instance to be a type error"
    );
}

#[test]
fn setter_call_on_immutable_instance_is_a_type_error() {
    // Calling a mutating (setter) method on an `=`-bound instance must fail type
    // checking; only a `:=` receiver may be mutated.
    let src = "Counter = {\n  value :: Num,\n  bump := (by :: Num) => < it.value := it.value + by >\n}\n^ = () -> Num => <\n  c = Counter { value = 30 }\n  c.bump(5)\n  c.value\n>";
    assert_type_error(src);
}

#[test]
fn an_immutable_method_writing_through_a_lambda_is_rejected() {
    // The contract is the DECLARATION: `=` promises the method does not mutate `it`.
    // A write from inside a lambda breaks that promise as surely as a direct one — the
    // write lands on the same receiver — so the verifier must see through the lambda.
    //
    let src = "T = {\n  v :: Num,\n  bump = () -> Num => <\n    [1, 2].each(x => it.v := x)\n    it.v\n  >\n}\n^ = () -> Num => <\n  t := T { v = 0 }\n  t.bump()\n>";
    assert_type_error(src);
}

#[test]
fn an_immutable_method_writing_through_a_declared_function_is_rejected() {
    // The write can also sit in a function DECLARED inside the body. That body is still
    // code the method runs against the same receiver, so it breaks the same promise —
    // the traversal must descend into a block's item declarations, not just its
    // expression statements.
    let src = "T = {\n  v :: Num,\n  bump = () -> Num => <\n    helper = () -> $ => < it.v := 99 >\n    helper()\n    it.v\n  >\n}\n^ = () -> Num => <\n  t := T { v = 0 }\n  t.bump()\n>";
    assert_type_error(src);
}

#[test]
fn an_immutable_method_calling_a_mutating_sibling_is_rejected() {
    // The transitive half: `viaBump` writes nothing itself, but calls a `:=` sibling on
    // `it`, so it mutates by proxy and cannot be declared `=`. Every sibling's contract is known from its
    // declaration, so this is a lookup rather than an inference.
    let src = "T = {\n  v :: Num,\n  bump := () -> $ => < it.v := 99 >,\n  viaBump = () -> Num => < it.bump() >\n}\n^ = () -> Num => <\n  t := T { v = 0 }\n  t.viaBump()\n>";
    assert_type_error(src);
}

#[test]
fn a_mutating_method_writing_through_a_lambda_runs_on_a_mutable_receiver() {
    // The other half of the rule: declaring it `:=` must actually let it work. On a
    // `:=` receiver it runs and mutates, leaving v = 2 — so the verifier rejects the
    // undeclared case without making the declared one uncallable.
    let src = "T = {\n  v :: Num,\n  bump := () -> Num => <\n    [1, 2].each(x => it.v := x)\n    it.v\n  >\n}\n^ = () -> Num => <\n  t := T { v = 0 }\n  t.bump()\n>";
    assert_exit(src, 2);
}

#[test]
fn a_method_is_a_setter_because_it_is_declared_one_not_because_it_writes() {
    // The contract is the declaration, not the body: `bump` writes nothing at all, yet
    // being declared `:=` still makes it require a `:=` receiver. Nothing else in the
    // suite would fail if registration quietly went back to inspecting bodies.
    let src = "T = {\n  v :: Num,\n  bump := () -> Num => < it.v >\n}\n^ = () -> Num => <\n  t = T { v = 0 }\n  t.bump()\n>";
    assert_type_error(src);
}

#[test]
fn an_operator_member_cannot_be_declared_mutating() {
    // An operator yields a value, and its dispatch never consults the setter set, so a
    // `:=` operator would promise a mutation no call site checks. Rejected at parse time,
    // with the rule rather than a stray-symbol complaint.
    let src = "Counter = {\n  value :: Num,\n  + := (other :: Counter) -> Num => < it.value >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let err = parser::parse(&tokens).expect_err("`:=` on an operator member must be rejected");
    assert!(
        err.message.contains("cannot be declared with `:=`"),
        "expected the operator-member rule, got: {}",
        err.message
    );
}

#[test]
fn the_render_member_cannot_be_declared_mutating() {
    // Same rule for the render member: it renders a value, and `print`/interpolation
    // never reach the receiver-mutability gate.
    let src =
        "Counter = {\n  value :: Num,\n  ` := () -> Text => < \"c\" >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let err = parser::parse(&tokens).expect_err("`:=` on the render member must be rejected");
    assert!(
        err.message.contains("cannot be declared with `:=`"),
        "expected the render-member rule, got: {}",
        err.message
    );
}

#[test]
fn a_sum_method_cannot_be_declared_mutating() {
    // A sum's data lives in variant payloads, reached by matching, and a match binding is
    // immutable — so there is no field to write and `:=` would declare a mutation nothing
    // can perform. Rejected at parse time, like an operator member.
    let src = "S = A(Num) / B {\n  poke := () -> $ => < it.x := 99 >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let err = parser::parse(&tokens).expect_err("`:=` on a sum method must be rejected");
    assert!(
        err.message.contains("cannot have a mutating method"),
        "expected a mutating-sum-method diagnostic, got: {}",
        err.message
    );
}

#[test]
fn a_field_write_on_a_sum_reports_the_missing_field_not_setter_advice() {
    // Diagnostic ORDER, not just presence. The mutation verifier must not run for sums:
    // if it did, `it.x := 99` on a sum would be answered with "declare it with ':='",
    // advice that leads nowhere — following it hits the type error below anyway. The
    // truthful complaint is that a sum has no such field.
    let src = "S = A(Num) / B {\n  poke = () -> $ => < it.x := 99 >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let message = checker
        .check_program(&program)
        .expect_err("a field write on a sum must be rejected")
        .to_string();
    assert!(
        !message.contains("declare it with `:=`"),
        "a sum field write must not be answered with setter advice, got: {message}"
    );
    assert!(
        message.contains("type mismatch"),
        "expected the field/type complaint, got: {message}"
    );
}

#[test]
fn an_immutable_method_that_mutates_names_the_binding_operator_to_change() {
    // The message is the whole remedy — it has to say which method and what to do — and
    // `docs/mutation.md` quotes it, so pin the wording rather than merely "some error".
    let src = "T = {\n  v :: Num,\n  bump = () -> $ => < it.v := 99 >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("an `=` method that mutates `it` must be rejected");
    let report = err
        .diagnostic()
        .render(&quilon::source_map::SourceMap::default(), false);
    assert!(
        report.contains("`T.bump`") && report.contains("help: declare it with `:=`"),
        "the diagnostic must name the method and the fix, got: {report}"
    );
}

#[test]
fn a_lambda_parameter_named_it_gets_a_shadowing_hint() {
    // `it` is an ordinary identifier, so a lambda parameter named `it` shadows the
    // receiver — the verifier still (conservatively) reports the write as a receiver
    // mutation, and the diagnostic must say why, so the reader can rename the parameter.
    let src = "P = { v :: Num }\nT = {\n  v :: Num,\n  poke = (ps :: []P) -> Num => <\n    ps.each(it => it.v := 5)\n    it.v\n  >\n}\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("the shadowed write is still reported as a receiver mutation");
    let message = err.to_string();
    assert!(
        message.contains("shadows the receiver") && message.contains("rename"),
        "the diagnostic must explain the `it` shadowing, got: {message}"
    );
}

#[test]
fn unannotated_method_parameter_is_rejected() {
    // No `Num` default any more: a method parameter must be annotated, exactly like an
    // ordinary function's — even when every call site happens to pass a `Num` (the case
    // the old default silently accepted).
    let src = "T = {\n  v :: Num,\n  add = (x) -> Num => < it.v + x >\n}\n^ = () -> Num => <\n  t = T { v = 1 }\n  t.add(41)\n>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("an unannotated method parameter must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("'x'") && message.contains("'T.add'") && message.contains("has no type"),
        "the diagnostic must name the parameter and the method, got: {message}"
    );
}

#[test]
fn static_call_on_a_method_that_reads_the_receiver_is_rejected() {
    // #259 part 1: a method called on the bare TYPE NAME (`Point.distance()`, not a
    // value) is legal only when the member never reads `it` (a STATIC method — the
    // natural spelling for a constructor). `distance` reads `it.x`, so this must be
    // rejected rather than pass the checker and crash at run time with "Undefined
    // variable: Point" (there is no value bound to a type's own name to pass as `it`).
    let src = "Point = { x :: Num, distance = () -> Num => < it.x > }\n^ = () -> Num => < Point.distance() >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("a static call on a receiver-reading method must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("`distance`")
            && message.contains("`Point.distance()`")
            && message.contains("`it`"),
        "the diagnostic must name the method, the call, and why (`it`), got: {message}"
    );
}

#[test]
fn setter_call_result_is_unit_not_num() {
    // A setter's body is a field write, which yields `$` (Unit) — so an unannotated
    // setter's result type is Unit, not Num. Using it in a Num position (`+ 1`) must
    // fail type checking, keeping the checker in agreement with codegen (a setter
    // call emits an i8/Unit, not an f64). Regression for a check/compile divergence.
    let src = "Counter = {\n  value :: Num,\n  bump := (by :: Num) => < it.value := it.value + by >\n}\n^ = () -> Num => <\n  c := Counter { value = 1 }\n  c.bump(5) + 1\n>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected using a setter's Unit result in a Num position to be a type error"
    );
}

#[test]
fn run_non_setter_method_on_immutable_instance_is_allowed() {
    // An `=`-declared method may be called on an `=`-bound (frozen) instance — only
    // `:=` methods need a `:=` receiver.
    assert_exit(
        "Counter = {\n  value :: Num,\n  peek = => < it.value >\n}\n^ = () -> Num => <\n  c = Counter { value = 42 }\n  c.peek()\n>",
        42,
    );
}

// --- A member call looks for the name on the receiver's type, never in the top level. ---

#[test]
fn run_method_wins_over_a_top_level_function_of_the_same_name() {
    // `c.bump(3)` asks `Counter` for `bump` and gets the method (5 + 3 = 8). The
    // top-level `bump` shares only the name; letting it claim the call passed the
    // receiver to a function that never expected it.
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump = (n :: Num) -> Num => < it.value + n >\n}\nbump = (n :: Num) -> Num => < n * 100 >\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.bump(3)\n>",
        8,
    );
}

#[test]
fn run_method_wins_over_a_same_named_overload_set() {
    // Same rule against an overload SET: two top-level `scale` members, neither of
    // which may answer `v.scale(3)` — `Vec`'s own method does (7 * 3 = 21).
    assert_exit(
        "Vec = {\n  x :: Num,\n  scale = (k :: Num) -> Num => < it.x * k >\n}\nscale = (n :: Num) -> Num => < 0 >\nscale = (t :: Text) -> Num => < 0 >\n^ = () -> Num => <\n  v :: Vec = Vec { x = 7 }\n  v.scale(3)\n>",
        21,
    );
}

#[test]
fn run_a_function_calling_a_same_named_method_is_not_a_self_call() {
    // Inside `bump`, the tail `c.bump(3)` is `Counter`'s method, not recursion — the
    // tail-call analysis must resolve the callee the same way call lowering does or it
    // rewrites the call into this function's own loop back-edge. 5 + 3 = 8.
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump = (n :: Num) -> Num => < it.value + n >\n}\nbump = (n :: Num) -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.bump(n)\n>\n^ = () -> Num => < bump(3) >",
        8,
    );
}

#[test]
fn a_member_call_never_falls_back_to_a_top_level_function() {
    // `Counter` declares no `bump`, so `c.bump(3)` is an error naming the type and the
    // member — never the top-level `bump`, which is a different function.
    let message = common::type_error_message(
        "Counter = {\n  value :: Num\n}\nbump = (n :: Num) -> Num => < n * 100 >\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.bump(3)\n>",
    );
    assert!(
        message.contains("Counter has no member `bump`"),
        "the diagnostic must name the receiver's type and the member, got: {message}"
    );
    // And it spells out the call that DOES reach that function, with the receiver the
    // reader wrote and an ellipsis for the arguments they passed.
    assert!(
        message.contains("help: call it as `bump(c, ...)`"),
        "the advice must spell out the plain call, got: {message}"
    );
}

#[test]
fn a_member_call_on_a_built_in_type_never_reaches_a_top_level_function() {
    // The rule is the receiver's TYPE, not just records: `Num` has no `double`, so the
    // top-level one does not answer `(5).double()`.
    let message = common::type_error_message(
        "double = (x :: Num) -> Num => < x * 2 >\n^ = () -> Num => < (5).double() >",
    );
    assert!(
        message.contains("Num has no member `double`"),
        "the diagnostic must name the receiver's type and the member, got: {message}"
    );
}

#[test]
fn a_free_call_still_reaches_a_top_level_function_over_a_same_named_method() {
    // Only the `recv.name(...)` form is receiver-scoped. `bump(3)` names the top-level
    // function (3 * 100 + 3 * 100 = 600).
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump = (n :: Num) -> Num => < it.value + n >\n}\nbump = (n :: Num) -> Num => < n * 100 >\n^ = () -> Num => < bump(3) + bump(3) >",
        600,
    );
}

#[test]
fn the_free_form_of_a_method_call_does_not_reach_the_method() {
    // A method is reached through its receiver and nowhere else: `bump(c, 3)` names the
    // top-level namespace, where this program has no `bump` at all.
    let message = common::type_error_message(
        "Counter = {\n  value :: Num,\n  bump = (n :: Num) -> Num => < it.value + n >\n}\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  bump(c, 3)\n>",
    );
    assert!(
        message.contains("no function `bump` in scope"),
        "the diagnostic must name the function that is missing, got: {message}"
    );
    // And it spells out the call that DOES reach the method, with the receiver written.
    assert!(
        message.contains("help: call it as `c.bump(...)`"),
        "the advice must spell out the member call, got: {message}"
    );
}

#[test]
fn a_built_in_method_does_not_answer_the_free_form() {
    // The rule is the same for the methods reserved on a built-in type: `split` belongs
    // to `Text`, so only `"a,b".split(",")` reaches it.
    let message = common::type_error_message(
        "^ = () -> Num => <\n  parts :: []Text = split(\"a,b\", \",\")\n  parts.size\n>",
    );
    assert!(
        message.contains("no function `split` in scope"),
        "the diagnostic must name the function that is missing, got: {message}"
    );
}

#[test]
fn a_method_and_a_top_level_function_of_one_name_each_answer_their_own_form() {
    // Both exist, and neither answers for the other: `c.bump(3)` is the method (5 + 3),
    // `bump(c, 3)` the top-level function (900 + 3). 8 + 903 = 911.
    assert_exit(
        "Counter = {\n  value :: Num,\n  bump = (n :: Num) -> Num => < it.value + n >\n}\nbump = (c :: Counter, n :: Num) -> Num => < 900 + n >\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.bump(3) + bump(c, 3)\n>",
        911,
    );
}

#[test]
fn a_tail_call_to_a_built_in_method_of_the_same_name_is_not_recursion() {
    // The tail `t.contains(s)` inside a top-level `contains` is `Text`'s built-in, which
    // declares no method symbol of its own — taking that miss for a self-call compiled it
    // into this function's loop back-edge and the program hung.
    assert_exit(
        "contains = (t :: Text, s :: Text) -> Bool => < t.contains(s) >\n^ = () -> Num => < contains(\"hello\", \"ell\") ? 7 : 3 >",
        7,
    );
}

#[test]
fn an_overload_set_below_the_call_is_reported_as_such() {
    // `contains` is a member of `Text`, but this program also defines a `contains` overload
    // set — below the call. The report has to name that, not send the reader to `Text`'s.
    let message = common::type_error_message(
        "^ = () -> Num => <\n  b :: Bool = contains(\"hi\", \"h\")\n  b ? 1 : 0\n>\ncontains = (t :: Text, s :: Text) -> Bool => < true >\ncontains = (t :: Text, n :: Num) -> Bool => < false >",
    );
    assert!(
        message.contains("before its definition"),
        "the diagnostic must say the definitions sit below the call, got: {message}"
    );
}

#[test]
fn a_name_rebound_in_an_inner_scope_is_not_still_the_outer_record() {
    // The receiver's type comes from the checker, which knows which binding a name refers
    // to. Reading it off a flat per-function map instead let the lambda's own `x` still
    // count as the `Foo` bound outside, sending `twice(x)` to `Foo`'s method. 10 + 7 = 17.
    assert_exit(
        "Foo = {\n  v :: Num,\n  twice = (n :: Num) -> Num => < 999 >\n}\ntwice = (n :: Num) -> Num => < n * 2 >\n^ = () -> Num => <\n  x :: Foo = Foo { v = 7 }\n  doubled = [5].map(x => twice(x))\n  doubled[0] + x.v\n>",
        17,
    );
}

#[test]
fn a_top_level_function_named_like_a_mangled_method_is_not_that_method() {
    // Method dispatch asks what the type declares, not whether a symbol of the mangled
    // shape exists — otherwise a top-level `Counter_bump` answers `bump(c, 3)`. 5 + 3 = 8.
    assert_exit(
        "Counter = {\n  value :: Num\n}\nCounter_bump = (c :: Counter, n :: Num) -> Num => < 900 + n >\nbump = (c :: Counter, n :: Num) -> Num => < c.value + n >\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  bump(c, 3)\n>",
        8,
    );
}

#[test]
fn a_method_may_be_named_like_a_compiler_provided_form() {
    // `assert` and the sum constructors are top-level names, so a member call reaches
    // neither: `c.assert(3)` is `Counter`'s own method. 5 + 3 = 8.
    assert_exit(
        "Counter = {\n  value :: Num,\n  assert = (n :: Num) -> Num => < it.value + n >\n}\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.assert(3)\n>",
        8,
    );
}

#[test]
fn a_method_answers_a_member_call_at_an_output_built_ins_own_arity() {
    // `c.print()` is `print(c)`, exactly the arity `print` claims — and it is still a member
    // call, so `Counter`'s own `print` answers it in both passes. Letting the built-in claim
    // it in codegen alone is how the checker and codegen come apart. 5 + 1 = 6.
    assert_exit(
        "Counter = {\n  value :: Num,\n  print = () -> Num => < it.value + 1 >\n}\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.print()\n>",
        6,
    );
}

#[test]
fn a_member_call_never_reaches_an_output_built_in() {
    // A type with a render member is printable — but through `print(c)`, the top-level form.
    // `c.print()` asks `Counter` for a `print` it does not have, and the advice names the
    // compiler-provided one rather than pretending nothing of that name exists.
    let message = common::type_error_message(
        "<< core.io\nCounter = {\n  value :: Num,\n  ` = () -> Text => < \"Counter(`it.value`)\" >\n}\n^ = () -> Num => <\n  c :: Counter = Counter { value = 5 }\n  c.print()\n  0\n>",
    );
    assert!(
        message.contains("Counter has no member `print`")
            && message.contains("help: call it as `io.print(c)` under `<< core.io`"),
        "expected the diagnostic to name the member and point at the built-in, got: {message}"
    );
}

#[test]
fn an_unknown_member_with_no_function_of_that_name_suggests_nothing() {
    // A method calling a sibling declared below it (there is no hoisting inside a type)
    // gets the plain error — pointing at a top-level function that does not exist would
    // be advice that fails too.
    let message = common::type_error_message(
        "Counter = {\n  value :: Num,\n  down = (n :: Num) -> Num => < n <= 0 ? it.value : it.down(n - 1) >\n}\n^ = () -> Num => < 0 >",
    );
    assert!(
        message.contains("Counter has no member `down`") && !message.contains("call it as"),
        "expected the bare diagnostic with no call-it-as advice, got: {message}"
    );
}

// --- Unit type (`$`): the type and its sole value share the symbol `$`. ---

#[test]
fn run_entry_returns_unit_exits_zero() {
    // `^` typed `-> $` with the unit value `$` as its body: a non-Num body, so
    // the entry-point wrapper coerces it to exit code 0.
    assert_exit("^ = () -> $ => < $ >", 0);
}

#[test]
fn run_function_returning_unit() {
    // A non-entry function may be typed `-> $`; calling it then exiting with a
    // Num keeps the program's exit code under control.
    assert_exit(
        "noop = () -> $ => < $ >\n^ = () -> Num => <\n  noop()\n  7\n>",
        7,
    );
}

#[test]
fn run_print_yields_unit_usable_where_unit_expected() {
    // `print(...)` returns `$`, so it type-checks as the body of a `-> $` function.
    assert_exit_linked(
        "<< core.io\nlog = (m :: Text) -> $ => < io.print(m) >\n^ = () -> Num => <\n  log(\"hi\")\n  0\n>",
        0,
    );
}

#[test]
fn run_eprint_returns_unit_as_last_expression() {
    // `eprint` returns `$`; as the entry point's last expression (no trailing 0)
    // the non-Num body coerces to exit 0.
    assert_exit_linked("<< core.io\n^ = () => < io.eprint(\"oops\") >", 0);
}

#[test]
fn run_unannotated_print_wrapper_compiles_and_runs() {
    // Regression: `log = (m :: Num) => io.print(m)` has no return annotation; its body is a
    // `print` call, which returns `$` (Unit). Codegen must infer the `$` return
    // type (i8) rather than defaulting to Num (f64), or the generated function
    // would `ret i8` into an f64 signature and fail LLVM module verification.
    assert_exit_linked(
        "<< core.io
log = (m :: Num) => < io.print(m) >
^ = () -> Num => <
  log(5)
  0
>",
        0,
    );
}

// A function/method body whose last statement is a declaration
// (`=`/`:=`) has no expression tail to type from — `docs/expressions/README.md` § Blocks
// says the block itself evaluates to `$`. Codegen must take that from the checker's
// type-oracle (the body's recorded type) rather than guessing from the tail's syntax,
// both for the LLVM return type and for the block's own emitted value.

#[test]
fn run_entry_unit_ending_in_declaration() {
    // The entry point itself: body ending in a declaration, non-Num, so it implicitly
    // exits 0 — this form already worked before the fix; kept as a baseline.
    assert_exit("^ = () -> $ => < x = 1 >", 0);
}

#[test]
fn run_function_annotated_unit_ending_in_declaration() {
    assert_exit(
        "f = () -> $ => < x = 1 >\n^ = () -> Num => <\n  f()\n  0\n>",
        0,
    );
}

#[test]
fn aot_function_annotated_unit_ending_in_declaration() {
    if !tool_available("clang") {
        eprintln!("skipping the native block-Unit check: clang is not on PATH");
        return;
    }
    let src = "f = () -> $ => < x = 1 >\n^ = () -> Num => <\n  f()\n  0\n>";
    let (code, _) = build_and_run_native("unit_block_declaration_tail", src);
    assert_eq!(code, 0, "a native build must exit 0 on the same program");
}

#[test]
fn run_function_inferred_unit_ending_in_declaration() {
    // An UNANNOTATED function whose body ends in a declaration infers `$` from the
    // checker's recorded body type, not `Num`.
    assert_exit("g = () => < x = 1 >\n^ = () -> Num => <\n  g()\n  0\n>", 0);
}

#[test]
fn run_function_unit_ending_in_mutable_declaration() {
    // `:=` as a function body's last statement types as `$`, same as `=`.
    assert_exit(
        "f = () -> $ => < x := 1 >\n^ = () -> Num => <\n  f()\n  0\n>",
        0,
    );
}

#[test]
fn run_local_function_unit_ending_in_declaration() {
    // A function declared INSIDE another function's body (here `^`'s) is lowered the
    // same way as a top-level one.
    assert_exit(
        "^ = () -> Num => <\n  local = () -> $ => < x = 1 >\n  local()\n  0\n>",
        0,
    );
}

#[test]
fn run_record_method_unit_ending_in_declaration() {
    assert_exit(
        "T = {\n  v :: Num,\n  m = () -> $ => < x = 1 >\n}\n^ = () -> Num => <\n  t = T { v = 1 }\n  t.m()\n  0\n>",
        0,
    );
}

#[test]
fn run_sum_method_unit_ending_in_declaration() {
    assert_exit(
        "Shape = Circle(Num) / Square(Num) {\n  m = () -> $ => < x = 1 >\n}\n^ = () -> Num => <\n  s = Circle(1)\n  s.m()\n  0\n>",
        0,
    );
}

#[test]
fn unit_is_incompatible_with_num() {
    // `$` has type Unit, which is not Num — annotating a Num return with a `$`
    // body must fail type checking.
    let src = "^ = () -> Num => < $ >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected `$` (Unit) body for a `-> Num` function to be a type error"
    );
}

// --- No silent `Num` default: an uninferable type is a compile error. ---

#[test]
fn unannotated_recursive_function_needs_a_return_type() {
    // A self-recursive call needs to already know what the function returns; an
    // unannotated function's return type isn't known until its body — which the
    // recursive call sits inside — is fully checked. This used to silently assume
    // `Num` for the recursive call, wrongly rejecting a valid non-`Num`-returning
    // recursive function with a confusing `Type mismatch` instead of naming the real
    // problem.
    let message = common::type_error_message(
        "f = (n :: Num) => < n <= 0 ? \"done\" : f(n - 1) >\n^ = () -> Num => < 0 >",
    );
    assert!(
        message.contains("recursive function 'f'") && message.contains("annotated return type"),
        "expected a clear recursive-return-type diagnostic, got: {message}"
    );
}

#[test]
fn annotated_recursive_function_with_a_non_num_return_type_works() {
    // The exact shape the old `Num` placeholder used to wrongly reject: a recursive
    // function returning `Text`, not `Num`. Annotating it resolves the recursive call's
    // type correctly and the program runs.
    assert_exit(
        "f = (n :: Num) -> Text => < n <= 0 ? \"done\" : f(n - 1) >\n\
         ^ = () -> Num => < f(3).size >",
        4,
    );
}

#[test]
fn empty_block_is_a_compile_error() {
    // A `< >` block with no statements at all has nothing that ran and nothing to
    // evaluate to — a compile error, not a silent `Num` (or `$`).
    let message = common::type_error_message("^ = () -> Num => <\n>");
    assert!(
        message.contains("no value") && message.contains("no statements"),
        "expected a clear empty-block diagnostic, got: {message}"
    );
}

#[test]
fn block_ending_in_a_reassignment_is_unit_not_a_silent_num() {
    // A block whose LAST statement is a declaration (`:=`/`=`), not an expression, has
    // no value to hand back — it types as `$` (Unit), like an `it.field :=` method body,
    // not a silently-assumed `Num`. This is the `.each` side-effecting-lambda idiom.
    assert_exit(
        "^ = () -> Num => <\n  sum := 0\n  [1, 2, 3].each(x => <\n    sum := sum + x\n  >)\n  sum\n>",
        6,
    );
}

#[test]
fn block_ending_in_a_reassignment_is_incompatible_with_num() {
    // The `$` a declaration-ending block types as is not `Num` — using it where a `Num`
    // is expected must fail, exactly like any other `$`/`Num` mismatch.
    assert_type_error("^ = () -> Num => <\n  x := 1\n>");
}

#[test]
fn empty_array_literal_without_context_is_a_compile_error() {
    // `[]` alone has no element type, and nothing here states one — a compile error
    // naming the literal, not a silent `[]Num`.
    let message = common::type_error_message("^ = () -> Num => <\n  xs = []\n  0\n>");
    assert!(
        message.contains("empty array literal") && message.contains("no element type"),
        "expected a clear empty-array diagnostic, got: {message}"
    );
}

#[test]
fn empty_array_literal_infers_from_binding_annotation() {
    // The binding's own annotation supplies the element type — here `Text`, so a `Num`
    // default would have failed to compile (`.size` on an empty `[]Text` is fine; the
    // point is the element type is genuinely `Text`, not `Num`).
    assert_exit("^ = () -> Num => <\n  xs :: []Text = []\n  xs.size\n>", 0);
}

#[test]
fn empty_array_literal_infers_from_call_argument_parameter_type() {
    // `count`'s declared `[]Text` parameter type seeds the empty literal argument.
    assert_exit(
        "count = (xs :: []Text) -> Num => < xs.size >\n^ = () -> Num => < count([]) >",
        0,
    );
}

#[test]
fn empty_array_literal_infers_from_declared_return_type() {
    // The function's own `-> []Text` annotation seeds its empty-literal body.
    assert_exit(
        "empty = () -> []Text => < [] >\n^ = () -> Num => < empty().size >",
        0,
    );
}

#[test]
fn empty_map_literal_without_context_is_a_compile_error() {
    let message = common::type_error_message("^ = () -> Num => <\n  m = [|=>|]\n  0\n>");
    assert!(
        message.contains("empty map literal") && message.contains("no key/value type"),
        "expected a clear empty-map diagnostic, got: {message}"
    );
}

#[test]
fn empty_map_literal_infers_non_num_types_from_binding_annotation() {
    // `Text => Bool`, not `Num => Num` — proves the type comes from the annotation, not
    // a coincidental default.
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Bool|] = [|=>|]\n  m.size\n>",
        0,
    );
}

#[test]
fn empty_set_literal_without_context_is_a_compile_error() {
    let message = common::type_error_message("^ = () -> Num => <\n  s = [||]\n  0\n>");
    assert!(
        message.contains("empty set literal") && message.contains("no element type"),
        "expected a clear empty-set diagnostic, got: {message}"
    );
}

#[test]
fn empty_set_literal_infers_non_num_type_from_binding_annotation() {
    assert_exit("^ = () -> Num => <\n  s :: [|Text|] = [||]\n  s.size\n>", 0);
}

// --- Type-annotated bindings inside a `< >` block (parity with top-level). ---

#[test]
fn block_level_annotated_bindings_parse_and_run() {
    // `name :: Type = expression` must work INSIDE a block exactly as at top level. Covers
    // Num, Text, Bool, and an array (`[]Num`) annotation.
    assert_exit(
        "^ = () -> Num => <\n  n :: Num = 5\n  t :: Text = \"abcd\"\n  ok :: Bool = t.size == 4\n  xs :: []Num = [1, 2, 3]\n  ok ? n + t.size + xs.size : 0\n>",
        12,
    );
}

#[test]
fn block_level_annotated_binding_wrong_type_is_a_type_error() {
    // A block-level annotation must be enforced just like a top-level one.
    assert_type_error("^ = () -> Num => <\n  x :: Text = 5\n  0\n>");
}

// --- Ad-hoc overloading: exact-type dispatch over an overload set. ---

#[test]
fn run_overload_set_resolves_by_argument_type() {
    // Two `pick` definitions form an overload set; each call resolves to the member
    // whose parameter type matches exactly. The Num and Text members do different
    // things, so the exit code proves the right one ran for each call.
    assert_exit(
        "pick = (n :: Num) -> Num => < n + 1 >\npick = (s :: Text) -> Num => < s.size >\n^ = () -> Num => < pick(40) + pick(\"ab\") >",
        43,
    );
}

#[test]
fn run_operator_overload_on_user_type() {
    // A `==` operator MEMBER of a record type (`it` = left operand, the one parameter =
    // right); resolved like any operator overload and lowered to a direct call. Returns
    // Bool, used in a ternary.
    assert_exit(
        "P = { x :: Num, y :: Num, == = (other :: P) -> Bool => < it.x == other.x && it.y == other.y >}\n^ = () -> Num => < P { x = 1, y = 2 } == P { x = 1, y = 2 } ? 42 : 0 >",
        42,
    );
}

#[test]
fn comparison_operator_overload_must_return_bool() {
    // A comparison/equality operator member is a predicate — a non-Bool return type
    // is a compile error. (Arithmetic operators have no such constraint.)
    assert_type_error("V = { x :: Num, == = (other :: V) -> V => < it >}\n^ = () -> Num => < 0 >");
}

#[test]
fn run_operator_overload_returning_record_survives_frame() {
    // A `+` operator member that RETURNS a record: the record is GC-allocated, so its
    // fields are still readable after the operator call returns (would dangle if it
    // were a stack alloca). Subsequent expressions must not corrupt it.
    assert_exit(
        "V = { x :: Num, y :: Num, + = (other :: V) -> V => < V { x = it.x + other.x, y = it.y + other.y } >}\n^ = () -> Num => <\n  v = V { x = 1, y = 2 } + V { x = 30, y = 9 }\n  pad = 5 > 1 ? 0 : 99\n  v.x + v.y + pad\n>",
        42,
    );
}

#[test]
fn run_overloaded_operator_dispatch_uses_callee_return_type() {
    // Regression (review BUG1): codegen must infer a call's result from the callee's
    // declared return type, not default to Num — so `mkv(..) + mkv(..)` resolves the
    // user `(V, V)` `+` overload (it would otherwise fall to the numeric `+` and fail).
    assert_exit(
        "V = { x :: Num, y :: Num, + = (other :: V) -> V => < V { x = it.x + other.x, y = it.y + other.y } >}\nmkv = (n :: Num) -> V => < V { x = n, y = n } >\n^ = () -> Num => <\n  w = mkv(1) + mkv(20)\n  w.x + w.y\n>",
        42,
    );
}

#[test]
fn run_operator_member_after_expression_bodied_member_parses() {
    // An operator MEMBER on its own line, right after a method whose body is an
    // expression, must NOT be absorbed as a binary operator continuing that body — the
    // parser stops the member's body at the trailing `== =` / `+ =`.
    assert_exit(
        "P = { x :: Num, y :: Num,\n  sum = () -> Num => < it.x + it.y >\n  == = (other :: P) -> Bool => < it.x == other.x && it.y == other.y >\n}\n^ = () -> Num => < P { x = 1, y = 2 } == P { x = 1, y = 2 } ? P { x = 40, y = 2 }.sum() : 0 >",
        42,
    );
}

#[test]
fn run_overloaded_call_on_each_element_dispatches_by_element_type() {
    // Regression (review BUG4): an overloaded call on a `.each` element must
    // dispatch by the element type (Num here) — codegen tracks the element type.
    // The Text member would mis-handle a Num; resolving to the Num member yields
    // inc(11) = 12 for the final element. `last` is captured by reference (`:=`).
    assert_exit(
        "inc = (n :: Num) -> Num => < n + 1 >\ninc = (t :: Text) -> Num => < t.size >\n^ = () -> Num => <\n  last := 0\n  xs = [10, 20, 11]\n  xs.each(n => <\n    last := inc(n)\n  >\n  )\n  last\n>",
        12,
    );
}

#[test]
fn run_numeric_sum_payload_through_operator_overload() {
    // A numeric sum payload flows through an operator overload set: `Ok(x) => x * 2`
    // — `x` is a (generic) Result payload that resolves as Num for `*`, so it picks the
    // `(Num, Num)` member. (Guards the Generic-resolves-as-Num overload behavior.)
    assert_exit(
        "^ = () -> Num => <\n  r = Ok(21)\n  r ? | Ok(x) => x * 2 | NotOk(e) => 0\n>",
        42,
    );
}

#[test]
fn run_user_sum_payload_dispatches_overload_by_concrete_type() {
    // A user sum type's payloads carry CONCRETE types (unlike Result's generic ones), so
    // a match arm's payload binding dispatches an overloaded call by that concrete type.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\narea = (n :: Num) -> Num => < n * 3 >\n^ = () -> Num => <\n  s = Circle(14)\n  s ? | Circle(n) => area(n) | Rect(w, h) => w * h\n>",
        42,
    );
}

#[test]
fn run_user_sum_bool_payload_dispatches_to_bool_overload_member() {
    // Regression (review BUG3 family): a Bool payload binding must dispatch to the
    // `(Bool)` overload member — codegen tracks the binding's concrete type — not the
    // `(Num)` member (which previously produced an LLVM i1-into-f64 type mismatch).
    assert_exit(
        "Flag = On(Bool) / Off(Bool)\nclassify = (n :: Num) -> Num => < n + 1 >\nclassify = (b :: Bool) -> Num => < b ? 100 : 7 >\n^ = () -> Num => <\n  s = On(true)\n  s ? | On(b) => classify(b) | Off(b) => classify(b)\n>",
        100,
    );
}

#[test]
fn run_named_record_sum_payload_reads_field() {
    // A named RECORD nested as a sum variant's payload: construct `Box(Point{..})`,
    // match `Box(p)`, and read the record's fields back at their real type. 3 + 4 = 7.
    let src = r#"
Point = { x :: Num, y :: Num }
Boxed = Box(Point) / Empty

unwrap = (b :: Boxed) -> Num => <
  b ?
    | Box(p) => p.x + p.y
    | Empty  => 0
>

^ = () -> Num => < unwrap(Box(Point { x = 3, y = 4 })) >
"#;
    assert_exit(src, 7);
}

#[test]
fn run_sum_record_payload_reads_text_field_and_calls_method() {
    // The `Method`/`Post(Body)` shape: a record payload with a `Text` field and a method.
    // The bound payload keeps its record type, so a `Text` field round-trips (its grapheme
    // count) and a method call on the binding resolves. "hello".size = 5.
    let src = r#"
Body = { payload :: Text, len = () -> Num => < it.payload.size >}
Method = Get / Post(Body)

sizeOf = (m :: Method) -> Num => <
  m ?
    | Get     => 0
    | Post(b) => b.len()
>

^ = () -> Num => < sizeOf(Post(Body { payload = "hello" })) >
"#;
    assert_exit(src, 5);
}

#[test]
fn run_sum_record_payload_empty_variant_is_selected_by_tag() {
    // The nullary sibling of a record-payload variant still dispatches by tag alone.
    let src = r#"
Point = { x :: Num, y :: Num }
Boxed = Box(Point) / Empty

unwrap = (b :: Boxed) -> Num => <
  b ?
    | Box(p) => p.x + p.y
    | Empty  => 99
>

^ = () -> Num => < unwrap(Empty) >
"#;
    assert_exit(src, 99);
}

#[test]
fn reject_heterogeneous_record_and_num_payload_at_same_position() {
    // The "consistent payload type per position" invariant holds for named records too: a
    // record in one variant and a `Num` in another at the same slot is still rejected.
    let src = r#"
Point = { x :: Num, y :: Num }
Bad = Wrap(Point) / Plain(Num)
^ = () -> Num => < 0 >
"#;
    assert_type_error(src);
}

/// An array literal unifies the SAME variant's payload type across every element, not
/// just the first: `NotOk`'s payload is generic until the second element specializes it
/// to `Text`, and the unified element type must carry that specialization so a later
/// match on any element reads the real payload. "a".size + "bb".size = 1 + 2 = 3.
#[test]
fn run_array_of_results_unifies_ok_then_notok_text_payload() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok("a"), NotOk("bb")]
          rs.map(r => r ? | Ok(t) => t | NotOk(e) => e).reduce(0, (a, x) => a + x.size)
        >
    "#;
    assert_exit(src, 3);
}

/// The reversed element order: `NotOk` specializes first, `Ok` second. Unification must
/// not depend on which variant appears first.
#[test]
fn run_array_of_results_unifies_notok_then_ok_text_payload() {
    let src = r#"
        ^ = () -> Num => <
          rs = [NotOk("bb"), Ok("a")]
          rs.map(r => r ? | Ok(t) => t | NotOk(e) => e).reduce(0, (a, x) => a + x.size)
        >
    "#;
    assert_exit(src, 3);
}

/// Indexing a single element out of the unified array and matching it directly (no
/// `.map`) reads the same unified payload type. "bb".size = 2.
#[test]
fn run_indexed_element_of_unified_result_array_reads_specialized_payload() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok("a"), NotOk("bb")]
          rs[1] ? | Ok(t) => t.size | NotOk(e) => e.size
        >
    "#;
    assert_exit(src, 2);
}

/// `Bool` payloads unify across variants the same way `Text` does. `Ok(true)` maps to
/// `true`, `NotOk(false)` maps to `false`; one of the two mapped values is `true`.
#[test]
fn run_array_of_results_unifies_bool_payload_across_variants() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok(true), NotOk(false)]
          rs.map(r => r ? | Ok(b) => b | NotOk(b) => b).reduce(0, (a, x) => x ? a + 1 : a)
        >
    "#;
    assert_exit(src, 1);
}

/// `.each` reads every element's unified payload too, not just `.map`'s inline lambda —
/// the unification lives on the array's element type, not on any one consumer.
/// "a".size + "bb".size = 1 + 2 = 3.
#[test]
fn run_each_over_unified_result_array_reads_every_variants_payload() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok("a"), NotOk("bb")]
          total := 0
          rs.each(r => < total := total + (r ? | Ok(t) => t.size | NotOk(e) => e.size) >)
          total
        >
    "#;
    assert_exit(src, 3);
}

/// `Result`'s two variants may carry DIFFERENT concrete payload types — this is the
/// documented shape of `core.cli`'s `getOpt` (`Ok([]Text) / NotOk(Text)`), not a
/// restriction unification adds. An array literal mixing `Ok(Text)` and `NotOk(Num)`
/// keeps each variant's own concrete type. "hi".size + 3 = 2 + 3 = 5.
#[test]
fn run_array_literal_keeps_different_concrete_types_across_result_variants() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok("hi"), NotOk(3)]
          (rs[0] ? | Ok(t) => t.size | NotOk(n) => n)
            + (rs[1] ? | Ok(t) => t.size | NotOk(n) => n)
        >
    "#;
    assert_exit(src, 5);
}

/// Within the SAME variant, every element must still agree on a concrete payload type —
/// unification merges compatible types, it does not paper over a real conflict.
#[test]
fn reject_array_literal_mixing_concrete_types_within_the_same_variant() {
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok(1), Ok("a")]
          0
        >
    "#;
    assert_type_error(src);
}

/// AOT: the array-literal unification fix is not JIT-only.
#[test]
fn aot_array_of_results_unifies_variant_payloads() {
    if !tool_available("clang") {
        eprintln!("skipping the native array-unification check: clang is not on PATH");
        return;
    }
    let src = r#"
        ^ = () -> Num => <
          rs = [Ok("a"), NotOk("bb")]
          rs.map(r => r ? | Ok(t) => t | NotOk(e) => e).reduce(0, (a, x) => a + x.size)
        >
    "#;
    let (code, _) = build_and_run_native("array_result_unification", src);
    assert_eq!(code, 3, "a native build must exit 3 on the same program");
}

#[test]
fn reject_nested_sum_as_sum_payload() {
    // A named composite payload must be a RECORD. Nesting another SUM as a payload is not
    // supported and is rejected by the checker.
    let src = r#"
Inner = A / B
Outer = Wrap(Inner) / Bare
^ = () -> Num => < 0 >
"#;
    assert_type_error(src);
}

#[test]
fn reject_unknown_named_sum_payload() {
    // A payload naming a type that was never declared (no hoisting) is rejected, not
    // silently accepted as an empty record.
    let src = r#"
Bad = Wrap(Nope) / Bare
^ = () -> Num => < 0 >
"#;
    assert_type_error(src);
}

#[test]
fn run_text_equality_and_ordering_overloads() {
    // Built-in Text comparison overloads: `==` (equality) and `<`/`>` (lexicographic).
    assert_exit(
        "^ = () -> Num => <\n  eq = \"hi\" == \"hi\" ? 10 : 0\n  lt = \"abc\" < \"abd\" ? 20 : 0\n  gt = \"b\" > \"a\" ? 12 : 0\n  eq + lt + gt\n>",
        42,
    );
}

#[test]
fn run_text_inequality_is_false_when_equal() {
    // `!=` on Text is the negation of `==`.
    assert_exit("^ = () -> Num => < \"x\" != \"x\" ? 1 : 42 >", 42);
}

#[test]
fn run_bool_equality_compares_values() {
    // `Bool == Bool` (i1 operands) is a built-in `==` overload; it must codegen to an
    // integer compare, not error or miscompile.
    assert_exit("^ = () -> Num => < true == true ? 42 : 0 >", 42);
    assert_exit("^ = () -> Num => < true != false ? 42 : 0 >", 42);
}

#[test]
fn run_ok_dispatch_over_every_builtin_payload() {
    // `Ok` constructs over every built-in payload type, including `$` (zero-payload).
    // All four construct; the matched `Ok(Num)` extracts its payload as the exit code.
    assert_exit(
        "^ = () -> Num => <\n  n = Ok(42)\n  t = Ok(\"hello\")\n  b = Ok(true)\n  u = Ok($)\n  n ? | Ok(x) => x | NotOk(e) => 0\n>",
        42,
    );
}

#[test]
fn run_ok_text_payload_constructs_and_dispatches() {
    // `Ok(Text)` constructs and the match dispatches to the `Ok` arm by tag. (Using a
    // bound Text payload's fields is the separate, documented non-numeric-payload
    // limitation; here we only require construction + tag dispatch to work.)
    assert_exit(
        "^ = () -> Num => <\n  r = Ok(\"abcd\")\n  r ? | Ok(s) => 42 | NotOk(e) => 0\n>",
        42,
    );
}

#[test]
fn result_any_payload_crosses_a_generic_parameter() {
    // The uniform Result layout (`{ i8 tag, {ptr,i64} slot }`) lets a Result carrying ANY
    // payload — Num, Text, []Text, a Num NotOk — pass through a generic `(r :: Result)`
    // parameter that only matches by TAG. `isOk` returns 1 for Ok, 0 for NotOk; summing the
    // four calls yields 3 (three Ok, one NotOk).
    assert_exit(
        "isOk = (r :: Result) -> Num => < r ? | Ok(_) => 1 | NotOk(_) => 0 >\n\
         ^ = () -> Num => <\n\
         \x20 a = isOk(Ok(42))\n\
         \x20 b = isOk(Ok(\"hi\"))\n\
         \x20 c = isOk(Ok([\"x\", \"y\"]))\n\
         \x20 d = isOk(NotOk(7))\n\
         \x20 a + b + c + d\n\
         >",
        3,
    );
}

#[test]
fn result_composite_payload_round_trips_and_extracts() {
    // A composite-payload Result flows out of a `-> Result` function (whose concrete payload
    // type the checker propagates), and the caller matches + EXTRACTS the payload at its real
    // type: the `[]Text` payload's `.size` is read back as the exit code.
    assert_exit(
        "mk = () -> Result => < Ok([\"a\", \"b\", \"c\"]) >\n\
         ^ = () -> Num => < mk() ? | Ok(v) => v.size | NotOk(_) => 0 >",
        3,
    );
}

#[test]
fn result_notok_text_payload_extracts_through_boundary() {
    // A `NotOk(Text)` produced behind a `-> Result` boundary: the caller extracts the Text
    // payload and reads its length — proving a packed Text slot unpacks to a usable Text.
    assert_exit(
        "mk = () -> Result => < NotOk(\"boom\") >\n\
         ^ = () -> Num => < mk() ? | Ok(_) => 0 | NotOk(e) => e.size >",
        4,
    );
}

#[test]
fn result_bool_payload_round_trips() {
    // A `Bool` payload packs (zero-extended) into the slot and unpacks (truncated) back to a
    // usable `Bool`: `Ok(true)`'s payload gates the exit code.
    assert_exit(
        "mk = () -> Result => < Ok(true) >\n\
         ^ = () -> Num => < mk() ? | Ok(f) => (f ? 42 : 0) | NotOk(_) => 0 >",
        42,
    );
}

#[test]
fn result_user_sum_payload_boxes_crosses_and_extracts() {
    // A payload wider than the uniform `{ptr,i64}` slot — a user sum value `Circle(5)` — is
    // BOXED into the slot, so `Ok(Circle(5))` still crosses a generic `(r :: Result)` parameter
    // (isOk), and the caller extracts the sum value and matches it: `Rect(3,4)` -> 12.
    assert_exit(
        "Shape = Circle(Num) / Rect(Num, Num)\n\
         isOk = (r :: Result) -> Num => < r ? | Ok(_) => 1 | NotOk(_) => 0 >\n\
         ^ = () -> Num => <\n\
         \x20 a = isOk(Ok(Circle(5)))\n\
         \x20 area = Ok(Rect(3, 4)) ? | Ok(sh) => (sh ? | Circle(r) => r * r | Rect(w, h) => w * h) | NotOk(_) => 0\n\
         \x20 a + area\n\
         >",
        13,
    );
}

#[test]
fn result_nested_result_payload_boxes_and_extracts() {
    // A nested `Result` payload (also wider than the slot) boxes and unboxes: `Ok(Ok(7))`
    // crosses a generic parameter and the inner Num is extracted through both layers.
    assert_exit(
        "isOk = (r :: Result) -> Num => < r ? | Ok(_) => 1 | NotOk(_) => 0 >\n\
         ^ = () -> Num => <\n\
         \x20 a = isOk(Ok(Ok(7)))\n\
         \x20 inner = Ok(Ok(7)) ? | Ok(ir) => (ir ? | Ok(x) => x | NotOk(_) => 0) | NotOk(_) => 0\n\
         \x20 a + inner\n\
         >",
        8,
    );
}

#[test]
fn print_takes_every_builtin_type() {
    // One printing rule, not a member per type: each built-in renders through its own
    // `` ` `` and `print` yields `$`.
    assert_exit_linked(
        "<< core.io\n^ = () -> Num => <\n  io.print(1)\n  io.print(\"two\")\n  io.print(true)\n  0\n>",
        0,
    );
}

#[test]
fn a_user_print_is_unrelated_to_the_modules() {
    // The module's set is closed: the program's own `print` is an ordinary function
    // beside `io.print`, and each call reaches its own.
    assert_exit_linked(
        "<< core.io\nprint = (a :: Num, b :: Num) -> Num => < a + b >\n^ = () -> Num => <\n  io.print(\"hi\")\n  print(40, 2)\n>",
        42,
    );
}

// --- Negative overload cases: ambiguous / no-match are clear compile errors. ---

#[test]
fn no_matching_overload_is_a_compile_error() {
    // No `pick` overload accepts a Bool (exact-match, no coercion).
    assert_type_error(
        "pick = (n :: Num) -> Num => < n >\npick = (s :: Text) -> Num => < s.size >\n^ = () -> Num => < pick(true) >",
    );
}

#[test]
fn duplicate_overload_signature_is_a_compile_error() {
    // Two definitions with the SAME parameter types make every call ambiguous.
    assert_type_error(
        "pick = (n :: Num) -> Num => < n >\npick = (m :: Num) -> Num => < m + 1 >\n^ = () -> Num => < pick(1) >",
    );
}

#[test]
fn operator_with_no_overload_for_operand_types_is_a_compile_error() {
    // `+` has Num/Num and Text/Text overloads but none for Num + Bool.
    assert_type_error("^ = () -> Num => < 1 + true >");
}

// --- Entry-point `^` receiving `args :: []Text` and `env :: [|Text => Text|]`. ---
// Under the JIT, `args` is the exact slice the caller hands `run_program` (here the
// helpers pass a single-element argv, mirroring a native binary invoked with no extra
// args); `env` still comes from this process's real environment, which is not
// deterministic — so these tests assert only invocation-INDEPENDENT facts (argv[0] is
// always present, env size is non-negative). Caller-controlled argv parity is pinned
// directly by `jit_uses_caller_supplied_argv` below, and full JIT/AOT parity by the
// native tests in `tests/args_native_test.rs`.

#[test]
fn run_entry_with_args_parameter_typechecks_and_runs() {
    // `^(args :: []Text)` — `args.size` is always >= 1 (argv[0] is the program name),
    // so this is deterministic regardless of how the test harness was invoked.
    assert_exit(
        "^ = (args :: []Text) -> Num => < args.size >= 1 ? 7 : 0 >",
        7,
    );
}

#[test]
fn run_entry_with_args_and_env_parameters_runs() {
    // `^(args :: []Text, env :: [|Text => Text|])` — touches both the array and the Map;
    // the result depends only on invocation-independent facts (sizes), so it is deterministic.
    assert_exit(
        "^ = (args :: []Text, env :: [|Text => Text|]) -> Num => <\n  args.size >= 1 && env.size >= 0 ? 9 : 0\n>",
        9,
    );
}

#[test]
fn run_entry_indexes_into_args() {
    // Indexing `args[0]` yields a `Text`; binding it must type-check and run (the value
    // itself is non-deterministic, so we only assert the program completes -> exit 4).
    assert_exit(
        "^ = (args :: []Text) -> Num => <\n  first = args[0]\n  4\n>",
        4,
    );
}

#[test]
fn jit_uses_caller_supplied_argv() {
    // The JIT must thread the caller-supplied argv into `^`'s `args` verbatim — no
    // `quilon run <file>` CLI prefix leaked in. A program returning
    // `args.size` therefore returns exactly the length of the slice we pass, so this
    // is the JIT-side anchor for JIT/AOT argv parity: `run_program(&p, &[file, a, b, c])`
    // must equal a native `./file a b c` (which sees `args.size == 4`).
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let src = "^ = (args :: []Text) -> Num => < args.size >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let types = checker
        .check_program(&program)
        .expect("type checking failed");

    // `[<file>, a, b, c]` — exactly what `main.rs` builds for `quilon run f.qn a b c`.
    let argv = [
        "f.qn".to_string(),
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
    ];
    let defer = quilon::deferral::analyze(&program);
    let code = jit::run_program(
        &program,
        types.clone(),
        defer.clone(),
        common::no_sources(),
        &argv,
    )
    .expect("execution failed");
    assert_eq!(
        code, 4,
        "JIT `args.size` must equal the caller-supplied argv length (file + 3 user args)"
    );

    // A bare argv (`argv[0]` only) mirrors a native binary run with no extra args.
    let code = jit::run_program(
        &program,
        types,
        defer,
        common::no_sources(),
        &["f.qn".to_string()],
    )
    .expect("execution failed");
    assert_eq!(code, 1, "bare argv -> args.size == 1 (argv[0] only)");
}

#[test]
fn a_numeric_two_parameter_entry_is_rejected() {
    // `^(argc :: Num, argv :: Num)` is not an entry signature: an entry takes its
    // arguments as `args :: []Text`. It is rejected by the ordinary rule, with the
    // ordinary diagnostic — no special case of its own.
    let src = "^ = (argc :: Num, argv :: Num) -> Num => < argc >= 1 ? 3 : 0 >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("`^(argc :: Num, argv :: Num)` must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("unsupported signature"),
        "expected an unsupported-signature diagnostic, got: {message}"
    );
    assert!(
        !message.contains("legacy"),
        "the diagnostic must not offer the removed form, got: {message}"
    );
}

#[test]
fn entry_with_non_text_array_param_is_rejected() {
    // The runtime builds `Text` elements for the argv array, so an `^` whose first
    // parameter is `[]Num` (an array of a NON-`Text` element) must NOT be routed to the
    // argv arm — that would hand it mis-sized elements. The type checker rejects it up
    // front (so `quilon check` and `quilon run`/`build` all report the same clear
    // diagnostic) rather than silently miscompiling.
    let src = "^ = (args :: []Num) -> Num => < args.size >";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    let err = checker
        .check_program(&program)
        .expect_err("`^(args :: []Num)` must be rejected, not miscompiled");
    assert!(
        err.to_string().contains("unsupported signature"),
        "expected an unsupported-signature diagnostic, got: {err}"
    );
}

// --- The `>` lexing rule: `>` closes a block unless a same-line operand follows. ---

#[test]
fn run_bare_greater_than_works_on_one_line() {
    // `a > b` on a single line is the greater-than operator everywhere (no parens).
    assert_exit("^ = () -> Num => < 5 > 3 ? 42 : 0 >", 42);
}

#[test]
fn run_greater_than_inside_a_block() {
    // A `>` comparison used inside a `< >` block, whose own `>` still closes.
    assert_exit(
        "^ = () -> Num => <\n  ok = 10 > 2 ? 1 : 0\n  ok == 1 ? 42 : 0\n>",
        42,
    );
}

#[test]
fn run_block_bodied_lambda_as_a_call_argument_on_one_line() {
    // The closer sits directly before the call's `)`, where no operand can follow.
    assert_exit(
        "^ = () -> Num => <\n  total := 0\n  [1, 2, 3].each(x => < total := total + x >)\n  total * 7\n>",
        42,
    );
}

#[test]
fn run_greater_than_survives_next_to_a_closer() {
    // A `>` comparison as a call's last argument: the comparison keeps its operand,
    // and the `)` right after it is not mistaken for a block close.
    assert_exit(
        "^ = () -> Num => <\n  big = [3, 1, 4].filter(x => x > 2)\n  big.size * 21\n>",
        42,
    );
}

#[test]
fn dangling_comparison_at_line_end_is_an_error() {
    // A `>` with nothing after it on its line is a block close, so using it as a
    // comparison there must fail to parse (never silently miscompile).
    let src = "^ = () -> Num => <\n  x = 5\n  x >\n  3\n>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    assert!(
        parser::parse(&tokens).is_err(),
        "a `>` at the end of its line used as comparison must be a parse error"
    );
}

#[test]
fn unterminated_block_with_only_a_comparison_gt_is_an_error() {
    // The `>` here has an operand after it, so it is `Gt` and the block never closes
    // -> a clear parse error (unexpected EOF), not a silent miscompile.
    let src = "^ = () -> Num => < x > 5";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    assert!(
        parser::parse(&tokens).is_err(),
        "an unterminated block must be a parse error"
    );
}

#[test]
fn two_adjacent_closers_name_the_missing_space() {
    // `>>` is the export marker by maximal munch, so two closers need a space between
    // them. The diagnostic must say so rather than fail on a phantom export.
    let src = "^ = () -> Num => <\n  f = () => < 1 >>";
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let err = parser::parse(&tokens).expect_err("`>>` as two closers must be rejected");
    assert!(
        err.help
            .as_deref()
            .is_some_and(|help| help.contains("separate them with a space")),
        "expected the adjacent-closer hint, got: {:?}",
        err.help
    );

    // With the space, the same program parses.
    let spaced = "^ = () -> Num => <\n  f = () => < 1 > >";
    let tokens = Lexer::tokenize(spaced).expect("lexing failed");
    assert!(parser::parse(&tokens).is_ok(), "`> >` must parse");
}

#[test]
fn modulo_works_end_to_end() {
    // `%` was once documented and type-checked but had NO codegen arm — it passed
    // `check` and died at run/build with an internal error. It lowers to the f64
    // remainder (LLVM frem == C fmod).
    assert_exit("^ = () -> Num => < 7 % 3 >", 1);
}

#[test]
fn modulo_sign_follows_dividend() {
    // fmod semantics: the result takes the DIVIDEND's sign.
    assert_exit(
        "^ = () -> Num => < ((0 - 7) % 3 == 0 - 1 ? 1 : 0) + (7 % (0 - 3) == 1 ? 2 : 0) >",
        3,
    );
}

#[test]
fn modulo_handles_fractional_operands() {
    // One unified f64 Num: `%` must work on fractional operands too.
    assert_exit(
        "^ = () -> Num => < (7.5 % 2 == 1.5 ? 1 : 0) + (10 % 2.5 == 0 ? 2 : 0) >",
        3,
    );
}

#[test]
fn logical_operators_short_circuit() {
    // `&&`/`||` were once lowered as EAGER bitwise and/or — both operands always
    // ran. The side-effecting right operand must run only when the left does not
    // already decide the result: here only bump(4) and bump(8) may run.
    assert_exit(
        "^ = () -> Num => <\n  hits := 0\n  bump = (x :: Num) -> Bool => <\n    hits := hits + x\n    x > 0\n  >\n  a = false && bump(1)\n  b = true || bump(2)\n  c = true && bump(4)\n  d = false || bump(8)\n  hits\n>",
        12,
    );
}

#[test]
fn logical_operators_truth_table_unchanged() {
    // Short-circuit lowering must not change the VALUES: full truth table for the
    // decided and undecided paths of both operators. 1 + 8 + 32 = 41.
    assert_exit(
        "^ = () -> Num => <\n  t = (true && true ? 1 : 0) + (true && false ? 2 : 0) + (false && false ? 4 : 0) + (true || false ? 8 : 0) + (false || false ? 16 : 0) + (false || true ? 32 : 0)\n  t\n>",
        41,
    );
}

#[test]
fn short_circuit_guards_unchecked_indexing() {
    // The canonical guard idiom: with eager `&&` this performed an out-of-bounds read
    // of a[5]; with short-circuit it must skip the index entirely and take the else.
    assert_exit(
        "^ = () -> Num => <\n  a = [10, 20, 30]\n  i = 5\n  ok = i < a.size && a[i] == 10\n  ok ? 1 : 2\n>",
        2,
    );
}

#[test]
fn record_binding_name_reused_by_later_function_is_not_misrouted() {
    // `record_types`/`var_named_types` once accumulated across function emissions.
    // After `first` bound a RECORD to `p`, a later function taking an ARRAY
    // parameter `p` had its `p.size` diverted to the record-field path (a bogus
    // GEP read the array's first element -> 1). Each function must start from an
    // empty per-function frame.
    assert_exit(
        "first = () -> Num => <\n  p = { size = 5, other = 6 }\n  p.other\n>\nsecond = (p :: []Num) -> Num => < p.size >\n^ = () -> Num => <\n  x = first()\n  second([1, 2, 3])\n>",
        3,
    );
}

#[test]
fn closure_capturing_record_reads_its_field() {
    // A captured variable's type metadata must travel into the lifted lambda frame
    // WITH the capture (this once worked only because the enclosing frame's maps
    // leaked into the closure body's emission).
    assert_exit(
        "^ = () -> Num => <\n  r = { v = 7 }\n  f = () => < r.v >\n  f()\n>",
        7,
    );
}

#[test]
fn closure_capturing_named_record_calls_its_method() {
    // Method dispatch on a captured named-record value resolves through
    // `var_named_types`, which must be carried into the closure's frame.
    assert_exit(
        "Counter = { v :: Num, get = () -> Num => < it.v >}\n^ = () -> Num => <\n  c = Counter { v = 4 }\n  f = () => < c.get() >\n  f()\n>",
        4,
    );
}

#[test]
fn run_function_returning_array_is_usable() {
    // Regression: a user function whose declared return type is a bare array
    // (`[]Text` / `[]Num`) must yield the `{ptr,i64}` array VALUE, so a caller can
    // concatenate the result (`+`), take `.size`, and index it. Previously the return
    // was lowered to a bare `ptr`, so feeding it to `+`/`.size` panicked codegen.
    let src = r#"
        pair = (a :: Text, b :: Text) -> []Text => < [a, b] >
        nums = (n :: Num) -> []Num => < [n, n + 1, n + 2] >
        ^ = () -> Num => <
          xs :: []Text = pair("a", "bb") + pair("ccc", "d")
          ys :: []Num = nums(10)
          xs.size + xs[1].size + ys.size + ys[2]
        >
    "#;
    // xs = ["a","bb","ccc","d"] (size 4); xs[1] = "bb" (size 2);
    // ys = [10,11,12] (size 3); ys[2] = 12  ->  4 + 2 + 3 + 12 = 21.
    assert_exit(src, 21);
}

#[test]
fn run_closure_returning_array_is_usable() {
    // Regression: a local closure with an array return type must yield the `{ptr,i64}`
    // value (heap-backed), so its result concatenates and indexes — the SAME boundary
    // rule as top-level functions (both funnel through `boundary_type`).
    let src = r#"
        ^ = () -> Num => <
          mk := (n :: Num) -> []Num => < [n, n + 1] >
          xs :: []Num = mk(10) + mk(20)
          xs.size + xs[3]
        >
    "#;
    // mk(10) = [10,11], mk(20) = [20,21]; xs = [10,11,20,21] (size 4); xs[3] = 21 -> 25.
    assert_exit(src, 25);
}

#[test]
fn run_array_literal_survives_escaping_its_frame() {
    // Regression: an array literal stored in a record field and RETURNED must keep its
    // backing store alive after the defining frame dies. Here `make` returns a record
    // holding `[10,20,30]`; a later `clobber` call reuses the same stack region with a
    // fresh `[77,77,77]`. If the literal were stack-allocated, `p.xs` would dangle and
    // read `clobber`'s locals (observed exit 77); heap allocation makes it read 10.
    let src = r#"
        Pair = { xs :: []Num }
        make = () -> Pair => < Pair { xs = [10, 20, 30] } >
        clobber = (x :: Num) -> Num => <
          c = [x, x, x]
          c[0]
        >
        ^ = () -> Num => <
          p = make()
          z = clobber(77)
          b = p.xs
          b[0]
        >
    "#;
    assert_exit(src, 10);
}

#[test]
fn run_method_returning_array_is_usable() {
    // Regression: a record method with an array return type must yield the `{ptr,i64}`
    // value (heap-backed), usable with `.size` / indexing after the call returns.
    let src = r#"
        Bag = {
          tag :: Text,
          pair = () -> []Text => < [it.tag, it.tag] >
        }
        ^ = () -> Num => <
          b = Bag { tag = "hi" }
          ps :: []Text = b.pair()
          ps.size + ps[1].size
        >
    "#;
    // pair() = ["hi","hi"] (size 2); ps[1] = "hi" (size 2) -> 4.
    assert_exit(src, 4);
}

/// Regression (#194): a method parameter annotated with a user-defined RECORD type. The
/// checker used to compare the call site's unresolved annotation (`Named { fields: [] }`)
/// against the resolved argument type and reject two `P`s that print the same but compare
/// unequal; codegen then had no field types for a method parameter. t.v(1) + p.n(41) = 42.
#[test]
fn run_method_parameter_typed_as_a_user_record_resolves() {
    let src = r#"
        P = { n :: Num }
        T = {
          v :: Num,
          take = (p :: P) -> Num => < it.v + p.n >
        }

        ^ = () -> Num => <
          t = T { v = 1 }
          t.take(P { n = 41 })
        >
    "#;
    assert_exit(src, 42);
}

/// Regression (#194): the same shape with a SUM-typed method parameter — the acceptance
/// criteria's other required case. 1 (T.v) + 6*6 (Circle payload) = 37.
#[test]
fn run_method_parameter_typed_as_a_user_sum_resolves() {
    let src = r#"
        Shape = Circle(Num) / Square(Num)

        T = {
          v :: Num,
          area = (s :: Shape) -> Num => <
            s ? | Circle(r) => it.v + r * r
                | Square(side) => it.v + side * side
          >
        }

        ^ = () -> Num => <
          t = T { v = 1 }
          t.area(Circle(6))
        >
    "#;
    assert_exit(src, 37);
}

/// Regression (#194), AOT: the same record-typed-parameter method call must produce the
/// same result through `quilon build` as through the JIT — the checker/codegen fix is not
/// JIT-only.
#[test]
fn aot_method_parameter_typed_as_a_user_record_resolves() {
    if !tool_available("clang") {
        eprintln!("skipping the native method-parameter check: clang is not on PATH");
        return;
    }
    let src = r#"
        P = { n :: Num }
        T = {
          v :: Num,
          take = (p :: P) -> Num => < it.v + p.n >
        }

        ^ = () -> Num => <
          t = T { v = 1 }
          t.take(P { n = 41 })
        >
    "#;
    let (code, _) = build_and_run_native("method_param_user_record", src);
    assert_eq!(code, 42, "a native build must exit 42 on the same program");
}

/// Regression (#259): an instance method constructing a fresh value of its OWN type. The
/// checker's `self`-referential return type used to leave codegen unable to resolve
/// `p.twin().x` — a field access whose base is a call result rather than a plain
/// variable. Point { x = 5 }.twin().x = 5.
#[test]
fn run_method_returning_a_fresh_value_of_its_own_type() {
    let src = r#"
        Point = { x :: Num, twin = () -> Point => < Point { x = it.x } > }
        ^ = () -> Num => < p = Point { x = 5 }  p.twin().x >
    "#;
    assert_exit(src, 5);
}

/// Regression (#259 part 1): a STATIC method (one whose body never reads `it`) called on
/// the bare TYPE NAME — the natural spelling for a constructor (`Request.get(url)`).
/// `origin` returns a fresh `Point` and takes an argument, exercising both the checker's
/// type-name-receiver detection and codegen's placeholder-receiver call. 3 + 4 = 7.
#[test]
fn run_static_method_called_on_the_type_name_constructs_a_value() {
    let src = r#"
        Point = {
          x :: Num,
          y :: Num,
          at = (n :: Num) -> Point => < Point { x = n, y = n + 1 } >
        }
        ^ = () -> Num => < p = Point.at(3)  p.x + p.y >
    "#;
    assert_exit(src, 7);
}

/// AOT counterpart: the static call must produce the same result through `quilon build`.
#[test]
fn aot_static_method_called_on_the_type_name_constructs_a_value() {
    if !tool_available("clang") {
        eprintln!("skipping the native static-method check: clang is not on PATH");
        return;
    }
    let src = r#"
        Point = {
          x :: Num,
          y :: Num,
          at = (n :: Num) -> Point => < Point { x = n, y = n + 1 } >
        }
        ^ = () -> Num => < p = Point.at(3)  p.x + p.y >
    "#;
    let (code, _) = build_and_run_native("static_method_ctor", src);
    assert_eq!(code, 7, "a native build must exit 7 on the same program");
}

/// A SUM type's trailing `{ }` methods block may declare a static member too — the same
/// static-eligibility rule (never reads `it`) applies regardless of whether the receiver
/// type is a record or a sum. `"a shape"`.size = 7.
#[test]
fn run_static_method_on_a_sum_types_trailing_methods_block() {
    let src = r#"
        Shape = Circle(Num) / Square(Num) {
          describe = () -> Text => < "a shape" >
        }
        ^ = () -> Num => < Shape.describe().size >
    "#;
    assert_exit(src, 7);
}

/// A static call reached through a MODULE BINDING: `<< "point_lib.qn"` binds `point_lib`,
/// and after import qualification the type's own name is `point_lib.Point` — the checker's
/// and codegen's type-name-receiver detection must still line up on the QUALIFIED name,
/// not just a bare local type. `point_lib.Point.origin()` builds `{x=0,y=0}`; 0 + 0 = 0.
#[test]
fn run_static_method_called_through_a_module_binding() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let src = r#"
        << "point_lib.qn"
        ^ = () -> Num => <
          p = point_lib.Point.origin()
          p.x + p.y
        >
    "#;
    assert_exit_linked_from(src, &fixtures, 0);
}

/// A static-eligible method (one that never reads `it`) is unaffected when called on an
/// ordinary VALUE instead of the type name — static-eligibility only gates the type-name
/// receiver shape, never restricts the value-receiver form. `q.origin()` on an existing
/// `Point` still builds a fresh `{x=0,y=0}`.
#[test]
fn run_static_eligible_method_called_on_a_value_is_unaffected() {
    let src = r#"
        Point = { x :: Num, y :: Num, origin = () -> Point => < Point { x = 0, y = 0 } > }
        ^ = () -> Num => <
          p = Point { x = 1, y = 2 }
          q = p.origin()
          q.x + q.y
        >
    "#;
    assert_exit(src, 0);
}

/// Regression (#257): a type declared INSIDE a block (nested in `^`'s body, not at the
/// top level) whose method calls an `@` leaf IO primitive. Both AST walkers used to skip
/// `Statement::Item(Item::TypeDeclaration(_))` entirely, and codegen's own type-declaration
/// emission unconditionally cleared `current_function` — either broke the enclosing body's
/// emission or (with a value-producing `@`) left the scheduler analysis blind to the call.
#[test]
fn run_nested_type_declared_inside_a_block_and_its_method_runs() {
    assert_exit_linked(
        r#"
<< core.time
^ = () -> Num => <
  Fetcher = { url :: Text, run = () -> Num => < @sleep(0.01)  42 > }
  f = Fetcher { url = "x" }
  f.run()
>
"#,
        42,
    );
}

#[test]
fn run_line_first_paren_is_new_statement() {
    // Statement-boundary rule end-to-end: without it, `x = f()` followed by the line
    // `(1 + 2)` fused into the call `f()(1 + 2)` ("Not a function" on the wrong
    // line). Now they are two statements, and the entry point exits with x = 7.
    let src = r#"
        f = () -> Num => < 7 >
        ^ = () -> Num => <
          x = f()
          (1 + 2)
          x
        >
    "#;
    assert_exit(src, 7);
}

#[test]
fn run_line_first_bracket_is_new_statement() {
    // Same for `[`: `b = a` followed by a line `[3, 4].each(...)` used to fuse into
    // the index `a[3, 4]`. Now `b` stays bound to `a` and the array line runs on its
    // own; the entry point exits with b[1] = 2.
    let src = r#"
        << core.io
        ^ = () -> Num => <
          a = [1, 2]
          b = a
          [3, 4].each(x => io.print(x))
          b[1]
        >
    "#;
    assert_exit_linked(src, 2);
}

#[test]
fn run_line_first_brace_is_new_statement() {
    // Same for `{`: `b = a` followed by a line `{ x = 1 }` used to fuse into the record
    // constructor `a { x = 1 }`. Now `b` stays bound to `a` and the brace line is its
    // own record statement; the entry point exits with b.x = 5, not 1.
    let src = r#"
        Point = { x :: Num }
        ^ = () -> Num => <
          a = Point { x = 5 }
          b = a
          { x = 1 }
          b.x
        >
    "#;
    assert_exit(src, 5);
}

#[test]
fn run_multiline_constructor_body_still_one_constructor() {
    // The rule gates only a LINE-FIRST `{`: a `{` opened on the type's line is still a
    // constructor, and its field body may span lines. Point { x=3, y=4 } -> 3 + 4 = 7.
    let src = r#"
        Point = { x :: Num, y :: Num }
        ^ = () -> Num => <
          p = Point {
            x = 3,
            y = 4
          }
          p.x + p.y
        >
    "#;
    assert_exit(src, 7);
}

#[test]
fn run_multiline_arguments_and_dot_chains_still_work() {
    // The rule gates only a LINE-FIRST `(` / `[`: an argument list opened on the
    // callee's line may span lines, and a continuation line starting with `.` still
    // chains. add(40, 2) = 42; [1,2,3,4] doubled -> filtered >4 -> 6+8 = 14; 42+14=56.
    let src = r#"
        add = (a :: Num, b :: Num) -> Num => < a + b >
        ^ = () -> Num => <
          sum = add(40,
            2)
          chained = [1, 2, 3, 4].map(x => x * 2)
            .filter(x => x > 4)
            .reduce(0, (acc, x) => acc + x)
          sum + chained
        >
    "#;
    assert_exit(src, 56);
}

/// A module's byte offsets restart at 0, so an importer's expression can occupy the
/// same byte range as one inside an imported module. Types are recorded per
/// expression for codegen to read back, keyed by source position — so if that key
/// ignores which file the position belongs to, the importer's `Num` answers for the
/// module's `Text` and the module's overloaded call dispatches to the wrong member.
///
/// The importer here is padded so that its `n` sits exactly on the `v` inside the
/// fixture's `kind(v)`. `classify("hi")` must still reach the Text member (2).
#[test]
fn importer_expression_on_a_modules_byte_range_does_not_retype_it() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let module_src = std::fs::read_to_string(fixtures.join("span_twin.qn")).unwrap();
    // The `v` argument of the module's `kind(v)` — the span to collide with.
    let target = module_src.find("kind(v)").expect("fixture shape changed") + "kind(".len();

    // Everything that precedes the `n` which has to land on `target`, with a comment
    // padded to push it exactly there. The padding is measured off the emitted prefix
    // itself, so the two cannot drift apart.
    let mut prefix = String::from("<< \"span_twin.qn\"\n^ = () -> Num => <\n  n = 7\n  ~ ");
    let assign = "\n  q = ";
    let pad = target
        .checked_sub(prefix.len() + assign.len())
        .expect("the fixture must leave room for the importer's preamble before `kind(v)`");
    prefix.push_str(&"p".repeat(pad));
    prefix.push_str(assign);

    let src = format!("{prefix}n\n  span_twin.classify(\"hi\") + q - 7\n>\n");
    // Without this holding, the test would pass without ever provoking a collision.
    assert!(
        src[target..].starts_with("n\n"),
        "the importer's `n` must land on the module's `v`"
    );

    assert_exit_linked_from(&src, &fixtures, 2);
}

/// An overload member may call itself once its return type is annotated: the member's
/// own definition is in scope for its body, so the recursive call resolves to it (and,
/// being in tail position, lowers to a loop). `p(3)` walks down to "done" — 4 bytes.
#[test]
fn run_recursive_overload_member_with_annotated_return() {
    assert_exit(
        "p = (n :: Num) -> Text => < n == 0 ? \"done\" : p(n - 1) >\np = (t :: Text) -> Num => < 0 >\n^ = () -> Num => < p(3).size >",
        4,
    );
}

/// A call resolves against the members defined above it: the first `pick` here answers
/// a Num argument, and the Text member added below is irrelevant to it.
#[test]
fn run_overload_call_uses_the_member_defined_above_it() {
    assert_exit(
        "pick = (n :: Num) -> Num => < 11 >\nfromNum = () -> Num => < pick(1) >\npick = (t :: Text) -> Num => < 22 >\n^ = () -> Num => < fromNum() + pick(\"x\") >",
        33,
    );
}

// --- The `core.time` primitives: `@sleep` (pause) and `now` (clock) ------------------
//
// `@sleep(seconds) -> $` is an effect-only pause: it waits on the current fiber, then
// execution continues in program order. `now()` reads a monotonic clock. These run a
// program that uses them through the full pipeline (front end + fiber scheduler) and assert
// it computes the right READY value and exits cleanly. They import `core.time`, so they go
// through the linked front end. Durations are tiny.

/// A `@sleep` statement runs (pausing the fiber) and the block then returns a ready value.
#[test]
fn run_sleep_pauses_then_returns_ready_value() {
    assert_exit_linked(
        r#"
<< core.time
^ = () -> Num => <
  @sleep(0.01)
  6 * 7
>
"#,
        42,
    );
}

/// Several sequential `@sleep` statements each run, and evaluation continues past them.
#[test]
fn run_multiple_sleeps_run_in_order() {
    assert_exit_linked(
        r#"
<< core.time
^ = () -> Num => <
  @sleep(0.01)
  @sleep(0.01)
  @sleep(0.01)
  5
>
"#,
        5,
    );
}

/// `@sleep` reached through an ordinary (unmarked) helper still runs on the entry fiber.
#[test]
fn run_sleep_through_a_helper_function() {
    assert_exit_linked(
        r#"
<< core.time
nap = () -> $ => < @sleep(0.01) >
^ = () -> Num => <
  nap()
  3
>
"#,
        3,
    );
}

/// `@sleep` also composes as a statement inside an ordinary array-method iteration.
#[test]
fn run_sleep_inside_each_iteration() {
    assert_exit_linked(
        r#"
<< core.time
^ = () -> Num => <
  [1, 2].each(n => @sleep(0.005))
  8
>
"#,
        8,
    );
}

/// `now()` deltas measure the pause: the elapsed time across a `@sleep` is at least the
/// requested duration (a sleep waits AT LEAST its duration, so `>=` is deterministic). The
/// program exits 0 only if `assert` held — genuine verification that the sleep waited.
#[test]
fn run_now_measures_that_sleep_actually_waited() {
    assert_exit_linked(
        r#"
<< core.test
<< core.time
^ = () -> Num => <
  start = time.now()
  @sleep(0.05)
  assert(time.now() - start >= 0.05, equals(true))
  0
>
"#,
        0,
    );
}
