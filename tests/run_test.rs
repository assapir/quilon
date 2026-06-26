// Execution-based tests: drive the full pipeline (lex -> parse -> typecheck ->
// codegen -> JIT) and assert the program's real exit code. This is the backbone
// that makes documented example behavior ("factorial(5) -> 120") actually verified.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::path::Path;
use std::sync::Mutex;

// LLVM's JIT and native-target initialization are not safe to run from multiple
// threads at once; cargo runs tests in parallel, so serialize execution here.
static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Compile and run `src`, asserting the entry point yields `expected`.
fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .expect("type checking failed");

    let code = jit::run_program(&program).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

/// Like `assert_exit`, but resolves `<<` imports (e.g. `<< core.io`) first, so
/// programs that use core-lib functions can be run end-to-end.
fn assert_exit_linked(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let program = quilon::modules::link(program, Path::new(".")).expect("import linking failed");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .expect("type checking failed");

    let code = jit::run_program(&program).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

#[test]
fn run_simple_arithmetic() {
    // examples/simple.ql
    assert_exit("^ = () -> Num => <\n  a = 5\n  b = 7\n  a + b\n>", 12);
}

#[test]
fn run_factorial() {
    // examples/factorial.ql -> factorial(5) = 120
    assert_exit(
        "factorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)\n\n^ = () -> Num => factorial(5)",
        120,
    );
}

#[test]
fn run_fibonacci() {
    // examples/fibonacci.ql -> fib(10) = 55
    assert_exit(
        "fib = (n :: Num) -> Num => n <= 1 ? n : fib(n - 1) + fib(n - 2)\n\n^ = () -> Num => fib(10)",
        55,
    );
}

#[test]
fn run_array_size() {
    // examples/array_size.ql -> [1,2,3,4,5].size = 5
    assert_exit(
        "^ = () -> Num => <\n  nums = [1, 2, 3, 4, 5]\n  nums.size\n>",
        5,
    );
}

#[test]
fn run_pattern_match_number() {
    // examples/option.ql -> matches the `5` arm
    assert_exit(
        "^ = () -> Num => <\n  value = 5\n  result = value ?\n    | 5 => 50\n    | 3 => 30\n    | _ => 0\n  result\n>",
        50,
    );
}

#[test]
fn run_pattern_match_wildcard() {
    // examples/pattern_wildcard.ql -> falls through to `_`
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
    assert_exit("^ = () -> Num => (\"héllo\" + \" 🌍\").size", 11);
}

#[test]
fn run_text_grapheme_length() {
    assert_exit("^ = () -> Num => (\"héllo\" + \" 🌍\").length", 7);
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

// --- Pipeline `|>` (first-arg injection) ---

#[test]
fn run_pipeline_chain() {
    // 10 |> double |> addFive  ==  addFive(double(10)) = 25
    assert_exit(
        "double = (x :: Num) -> Num => x * 2\naddFive = (x :: Num) -> Num => x + 5\n^ = () -> Num => 10 |> double |> addFive",
        25,
    );
}

#[test]
fn run_pipeline_injects_left_as_first_arg() {
    // 10 |> sub(3)  desugars to  sub(10, 3) = 7  (NOT sub(3, 10) = -7),
    // proving the left operand is injected as the FIRST argument.
    assert_exit(
        "sub = (a :: Num, b :: Num) -> Num => a - b\n^ = () -> Num => 10 |> sub(3)",
        7,
    );
}

// --- IO: write / print over `<< core.io` ---

#[test]
fn run_write_to_stdout_returns_byte_count() {
    // `"hi" |> write(stdout)` == `write("hi", stdout)`; write returns bytes written = 2.
    assert_exit_linked("<< core.io\n^ = () -> Num => \"hi\" |> write(stdout)", 2);
}

#[test]
fn run_print_text_then_exit() {
    // print writes "hello\n" to stdout and yields Num 0.
    assert_exit_linked(
        "<< core.io\n^ = () -> Num => <\n  print(\"hello\")\n  0\n>",
        0,
    );
}

// --- Loop: `for <pattern> <- <collection> => <body>` (decoupled from `|>`) ---

#[test]
fn run_for_loop_new_syntax_executes() {
    // The for-loop is side-effecting and yields Num 0; this proves the new
    // `for n <- coll => body` surface syntax parses, type-checks, and runs.
    assert_exit("^ = () -> Num => for n <- [1, 2, 3] => n", 0);
}

#[test]
fn run_for_loop_with_index_in_block() {
    assert_exit(
        "^ = () -> Num => <\n  for (val, i) <- [10, 20, 30] => <\n    x = val + i\n    x\n  >\n>",
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
    assert_exit("^ = () => \"done\"", 0);
}

#[test]
fn run_entry_num_body_still_is_exit_code() {
    // A Num body is unchanged: it becomes the exit code.
    assert_exit("^ = () -> Num => 42", 42);
}

#[test]
fn run_entry_side_effecting_main_no_trailing_zero() {
    // `<< core.io` + a print as the last expression, with NO trailing 0 -> exit 0.
    assert_exit_linked("<< core.io\n^ = () => print(\"hi\")", 0);
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
