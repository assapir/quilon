// Execution-based tests: drive the full pipeline (lex -> parse -> typecheck ->
// codegen -> JIT) and assert the program's real exit code. This is the backbone
// that makes documented example behavior ("factorial(5) -> 120") actually verified.

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
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
