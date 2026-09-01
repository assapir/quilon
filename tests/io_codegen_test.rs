// Codegen tests for the core IO builtins and the Boehm GC wiring.
//
// These exercise the code generator directly (no typecheck pass): `print`/`eprint`/
// `write` are recognized and lowered by codegen regardless of whether `core.io` has
// been imported. We assert the generated LLVM IR declares the right runtime
// intrinsics and that `main` initializes the GC.

use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser::parse;

fn gen_ir(source: &str) -> String {
    // The sources reach the output built-ins the only way a program can — through
    // `<< core.io` — so the link that resolves the qualified names runs here too.
    let source = format!("<< core.io\n{source}");
    let tokens = Lexer::tokenize(&source).unwrap();
    let program = parse(&tokens).unwrap();
    let program = quilon::modules::link(program, std::path::Path::new("."), None)
        .expect("import linking failed")
        .0;
    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "test");
    generator
        .generate(&program)
        .unwrap_or_else(|e| panic!("Codegen failed: {:?}", e))
}

#[test]
fn print_number_renders_via_num_to_text() {
    // `print` routes through the `` ` `` render operator: a Num renders to Text via
    // __num_to_text, then the Text is written with __print_text_fd.
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            io.print(42)
            0
        >
    "#,
    );
    assert!(
        ir.contains("@__num_to_text"),
        "expected __num_to_text call in:\n{ir}"
    );
    assert!(
        ir.contains("@__print_text_fd"),
        "expected __print_text_fd call in:\n{ir}"
    );
}

#[test]
fn print_text_lowers_to_print_text_fd_intrinsic() {
    // A `Text` is its `{ ptr, len }` pair, and the output path takes BOTH: the length is
    // what bounds the write, so no producer has to append anything past the bytes for
    // `print` to find the end.
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            io.print("hello")
            0
        >
    "#,
    );
    assert!(
        ir.contains("declare void @__print_text_fd(i64, ptr, i64)"),
        "expected a (fd, ptr, len) print signature in:\n{ir}"
    );
    let call = ir
        .lines()
        .find(|line| line.contains("call void @__print_text_fd"))
        .unwrap_or_else(|| panic!("expected a print call in:\n{ir}"));
    assert!(
        call.contains("(i64 1, ptr") && call.trim_end().ends_with(", i64 5)"),
        "expected stdout and the literal's 5-byte length: {call}"
    );
}

#[test]
fn write_lowers_to_write_bytes_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            io.write("hello", 1)
            0
        >
    "#,
    );
    assert!(
        ir.contains("@__write_bytes"),
        "expected __write_bytes in:\n{ir}"
    );
}

#[test]
fn print_bool_renders_via_bool_to_text() {
    // `print` routes through the `` ` `` render operator: a Bool renders to "True"/"False"
    // via __bool_to_text, then the Text is written with __print_text_fd.
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            io.print(true)
            0
        >
    "#,
    );
    assert!(
        ir.contains("@__bool_to_text"),
        "expected __bool_to_text in:\n{ir}"
    );
    assert!(
        ir.contains("@__print_text_fd"),
        "expected __print_text_fd in:\n{ir}"
    );
}

#[test]
fn main_wrapper_initializes_gc() {
    let ir = gen_ir(r#"^ = () -> Num => 0"#);
    assert!(
        ir.contains("__gc_init"),
        "expected GC init in main wrapper:\n{ir}"
    );
    // The GC init must be declared as an external (no body) function.
    assert!(ir.contains("declare") && ir.contains("@__gc_init"));
}

#[test]
fn main_wrapper_runs_a_pure_entry_on_a_fiber_too() {
    // Every program's entry runs on the scheduler: `main` hands a `__ql_entry` thunk to
    // `__run_fiber_main` and returns its result. This program is as pure as one gets — no
    // import, no call, no allocation, nothing that could park — so it holds the routing
    // independent of whether a program reaches an `@` primitive.
    let ir = gen_ir(r#"^ = () -> Num => 0"#);
    assert!(
        ir.contains("define i32 @__ql_entry("),
        "the entry dispatch belongs in a `__ql_entry` thunk:\n{ir}"
    );
    assert!(
        ir.contains("@__run_fiber_main("),
        "`main` must run the thunk on a scheduler fiber:\n{ir}"
    );
    assert!(
        ir.contains("@__ql_entry,") || ir.contains("@__ql_entry)"),
        "`__run_fiber_main` must be handed the thunk:\n{ir}"
    );
    // The dispatch lives in the thunk, so `main` itself never calls `^`.
    let main_body = ir
        .split("define i32 @main(")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .unwrap_or_else(|| panic!("no `main` in:\n{ir}"));
    assert!(
        !main_body.contains(r#"@"^""#),
        "`main` must reach `^` only through the fiber thunk, got:\n{main_body}"
    );
}

#[test]
fn color_enabled_lowers_to_the_color_intrinsic() {
    // `__color_enabled(fd)` is an INTERNAL compiler-lowered primitive (like `__exit`, and
    // exported by no module): it becomes a `__color_enabled` call, so `core.test` does not
    // have to guess at terminal detection in `.qn`.
    let ir = gen_ir("^ = () -> Num => __color_enabled(2) ? 1 : 0");
    assert!(
        ir.contains("declare i64 @__color_enabled(i64)"),
        "__color_enabled must lower to the runtime intrinsic, got:\n{ir}"
    );
}
