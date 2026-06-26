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
fn print_number_lowers_to_print_num_fd_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            print(42)
            0
        >
    "#,
    );
    assert!(
        ir.contains("@__print_num_fd"),
        "expected __print_num_fd call in:\n{ir}"
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
fn main_wrapper_initializes_gc() {
    let ir = gen_ir(r#"^ = () -> Num => 0"#);
    assert!(
        ir.contains("__gc_init"),
        "expected GC init in main wrapper:\n{ir}"
    );
    // The GC init must be declared as an external (no body) function.
    assert!(ir.contains("declare") && ir.contains("@__gc_init"));
}
