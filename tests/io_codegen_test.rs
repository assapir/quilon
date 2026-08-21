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
    let tokens = Lexer::tokenize(source).unwrap();
    let program = parse(&tokens).unwrap();
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
            print(42)
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
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            print("hello")
            0
        >
    "#,
    );
    assert!(
        ir.contains("@__print_text_fd"),
        "expected __print_text_fd in:\n{ir}"
    );
}

#[test]
fn write_lowers_to_write_bytes_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            write("hello", 1)
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
            print(true)
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
fn color_enabled_lowers_to_the_color_intrinsic() {
    // `colorEnabled(fd)` is a compiler-lowered core.io builtin (like `write`): it becomes
    // a `__color_enabled` call, so nothing in `.ql` has to guess at tty detection.
    let ir = gen_ir("^ = () -> Num => colorEnabled(2) ? 1 : 0");
    assert!(
        ir.contains("declare i64 @__color_enabled(i64)"),
        "colorEnabled must lower to the __color_enabled intrinsic, got:\n{ir}"
    );
}
