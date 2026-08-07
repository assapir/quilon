// Spread `<-` (prefix): splice a source's elements/fields into a literal.
//   Array : `[<-xs, 4, 5]`  -> every element of `xs`, then 4, 5.
//   Record: `{<-p, x = 9}`  -> a copy of `p` with `x` overridden (functional update).
//
// DISAMBIGUATION: `<-` is BOTH the infix inclusive-range operator (`lo <- hi`) and the
// prefix spread. They are told apart purely by POSITION: a `<-` that is the FIRST token
// of a `[ ]` element or `{ }` field is a spread; a `<-` that follows a complete
// expression is a range. So `[1 <- 4]` is a ONE-element array holding the range
// [1,2,3,4], while `[<-xs, 4]` splices xs. These tests prove both parse distinctly and
// drive the full pipeline (lex -> parse -> typecheck -> codegen -> JIT) for the runtime
// ones.

use quilon::ast::Expr;
use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::Mutex;

// LLVM JIT / target init isn't thread-safe; cargo runs tests in parallel.
static JIT_LOCK: Mutex<()> = Mutex::new(());

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

fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let mut checker = TypeChecker::new();
    assert!(
        checker.check_program(&program).is_err(),
        "expected a type error for source:\n{}",
        src
    );
}

// ---------------------------------------------------------------------------
// Parser-level disambiguation: same `<-` token, two meanings, by position.
// ---------------------------------------------------------------------------

/// Parse the initializer expression of the single `x = <expr>` binding in `src`.
fn parse_binding_value(src: &str) -> Expr {
    use quilon::ast::{Item, Statement};
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    // Find the `x = ...` var decl (top-level or inside the first block).
    for item in &program.items {
        if let Item::FunctionDecl(f) = item
            && let Expr::Block { stmts, .. } = &f.body
        {
            for stmt in stmts {
                if let Statement::Item(Item::VarDecl(d)) = stmt
                    && d.name == "x"
                {
                    return d.value.clone();
                }
            }
        }
    }
    panic!("no `x = ...` binding found");
}

/// `[1 <- 4]` — the `<-` follows a complete expression, so it is the RANGE operator:
/// a ONE-element array whose sole element is the range `1 <- 4`.
#[test]
fn bracket_range_is_a_one_element_array_of_a_range() {
    let v = parse_binding_value("^ = () -> Num => <\n  x = [1 <- 4]\n  0\n>");
    let Expr::Array { elements, .. } = v else {
        panic!("expected an array literal, got {v:?}");
    };
    assert_eq!(elements.len(), 1, "should be a single element (the range)");
    assert!(
        matches!(elements[0], Expr::Range { .. }),
        "the element should be a Range, got {:?}",
        elements[0]
    );
}

/// `[<-xs, 4]` — the leading `<-` is a SPREAD; `4` is an ordinary element.
#[test]
fn bracket_leading_arrow_is_a_spread() {
    let v = parse_binding_value("^ = () -> Num => <\n  xs = [1]\n  x = [<-xs, 4]\n  0\n>");
    let Expr::Array { elements, .. } = v else {
        panic!("expected an array literal, got {v:?}");
    };
    assert_eq!(elements.len(), 2);
    assert!(
        matches!(elements[0], Expr::Spread { .. }),
        "first element should be a Spread, got {:?}",
        elements[0]
    );
    assert!(
        !matches!(elements[1], Expr::Spread { .. }),
        "second element should be an ordinary element"
    );
}

/// `{<-p, x = 9}` — the leading `<-` field is a record SPREAD (functional update).
#[test]
fn brace_leading_arrow_is_a_record_spread() {
    let v =
        parse_binding_value("^ = () -> Num => <\n  p = { a = 1 }\n  x = { <-p, a = 9 }\n  0\n>");
    let Expr::Record { fields, .. } = v else {
        panic!("expected a record literal, got {v:?}");
    };
    assert_eq!(fields.len(), 2);
    assert!(
        matches!(fields[0].1, Expr::Spread { .. }),
        "first field should be a Spread, got {:?}",
        fields[0].1
    );
}

// ---------------------------------------------------------------------------
// Array spread — runtime behavior.
// ---------------------------------------------------------------------------

/// `[<-xs, 4, 5]` splices every element of `xs`, then appends 4, 5.
#[test]
fn array_spread_prepends_source_then_inline() {
    assert_exit(
        "^ = () -> Num => <\n  xs = [1, 2, 3]\n  ys = [<-xs, 4, 5]\n  ys.size\n>",
        5,
    );
}

/// A spread splices ELEMENTS, not the array itself: `[<-xs]` is a copy with the same
/// size, and its values are preserved in order.
#[test]
fn array_spread_only_is_a_copy() {
    assert_exit(
        "^ = () -> Num => <\n  xs = [7, 8, 9]\n  ys = [<-xs]\n  ys.size * 100 + ys[0] * 10 + ys[2]\n>",
        // size 3 -> 300, first 7 -> 70, last 9 -> 379
        379,
    );
}

/// Multiple spreads, left-to-right, interleaved with inline elements.
#[test]
fn array_multiple_spreads_left_to_right() {
    assert_exit(
        "^ = () -> Num => <\n  a = [1, 2]\n  b = [3, 4]\n  c = [0, <-a, <-b, 5]\n  c.size * 10 + c[0] + c[1] + c[5]\n>",
        // [0,1,2,3,4,5]: size 6 -> 60, +0 +1 +5 = 66
        66,
    );
}

/// Element repr is honored, not hardcoded to Num: a `[]Text` spread copies `{ptr,len}`
/// slots correctly, so the spliced texts round-trip.
#[test]
fn text_array_spread_preserves_elements() {
    assert_exit(
        "^ = () -> Num => <\n  hello = [\"ab\", \"c\"]\n  more = [<-hello, \"de\"]\n  more.size * 100 + more[0].size * 10 + more[2].size\n>",
        // size 3 -> 300, "ab".size 2 -> 20, "de".size 2 -> 2 => 322
        322,
    );
}

/// The disambiguation, executed: `[1 <- 4]` is a one-element array holding a range.
#[test]
fn bracket_range_runtime_is_nested_range() {
    assert_exit(
        "^ = () -> Num => <\n  r = [1 <- 4]\n  r.size * 10 + r[0].size\n>",
        // one element (the range) -> 10, inner range [1,2,3,4].size 4 => 14
        14,
    );
}

/// Spreading a non-array is a type error.
#[test]
fn array_spread_of_non_array_is_type_error() {
    assert_type_error("^ = () -> Num => <\n  n = 5\n  bad = [<-n]\n  0\n>");
}

// ---------------------------------------------------------------------------
// Record functional-update — runtime behavior.
// ---------------------------------------------------------------------------

/// `{<-p, x = 9}` copies all fields of `p`, overriding `x`; unmentioned fields survive.
#[test]
fn record_update_overrides_one_field() {
    assert_exit(
        "^ = () -> Num => <\n  p = { x = 1, y = 2 }\n  q = { <-p, x = 9 }\n  q.x * 10 + q.y\n>",
        // x overridden to 9, y copied 2 => 92
        92,
    );
}

/// A later entry overrides an earlier one (left-to-right): the last write of `x` wins.
#[test]
fn record_update_later_wins() {
    assert_exit(
        "^ = () -> Num => <\n  p = { x = 1 }\n  q = { <-p, x = 5, x = 8 }\n  q.x\n>",
        8,
    );
}

/// Precedence follows SOURCE ORDER, not override-vs-spread: an explicit field written
/// BEFORE a spread that also carries it loses to the spread (`{x = 9, <-p}` → `p.x`).
#[test]
fn record_update_spread_after_override_wins() {
    assert_exit(
        "^ = () -> Num => <\n  p = { x = 1, y = 2 }\n  q = { x = 9, <-p }\n  q.x * 10 + q.y\n>",
        // spread `<-p` comes AFTER `x = 9`, so x = p.x = 1, y = 2 => 12
        12,
    );
}

/// The mirror: an explicit field written AFTER a spread overrides it.
#[test]
fn record_update_override_after_spread_wins() {
    assert_exit(
        "^ = () -> Num => <\n  p = { x = 1, y = 2 }\n  q = { <-p, x = 9 }\n  q.x * 10 + q.y\n>",
        // x overridden to 9 after the spread, y = 2 => 92
        92,
    );
}

/// An override may ADD a new field; the result is an (anonymous) record with both.
#[test]
fn record_update_adds_field() {
    assert_exit(
        "^ = () -> Num => <\n  p = { a = 1 }\n  q = { <-p, b = 4 }\n  q.a * 10 + q.b\n>",
        14,
    );
}

/// A functional update of a NAMED record keeps the named type — and its methods.
#[test]
fn record_update_preserves_named_type_and_methods() {
    assert_exit(
        "Vec = {\n  x :: Num,\n  y :: Num,\n  sum = => it.x + it.y\n}\n^ = () -> Num => <\n  a = Vec { x = 10, y = 20 }\n  b = { <-a, x = 5 }\n  b.sum()\n>",
        // 5 + 20 = 25
        25,
    );
}

/// A record spread's Text fields round-trip through the copy.
#[test]
fn record_update_preserves_text_field() {
    assert_exit(
        "<< core.io\n^ = () -> Num => <\n  p = { name = \"Alice\", age = 30 }\n  q = { <-p, age = 31 }\n  print(q.name)\n  q.age\n>",
        31,
    );
}
