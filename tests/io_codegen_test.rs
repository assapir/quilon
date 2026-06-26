// Codegen tests for the core IO builtins and the Boehm GC wiring.
//
// These exercise the code generator directly (no typecheck pass): `print`/
// `println` are recognized and lowered by codegen regardless of whether the
// `core.io` module has been imported, and import/typecheck wiring lands in a
// sibling workstream. We assert the generated LLVM IR declares the right runtime
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
fn print_number_lowers_to_print_num_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            print(42)
            0
        >
    "#,
    );
    assert!(
        ir.contains("__print_num"),
        "expected __print_num call in:\n{ir}"
    );
    assert!(ir.contains("call") && ir.contains("@__print_num"));
}

#[test]
fn println_number_lowers_to_println_num_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            println(7)
            0
        >
    "#,
    );
    assert!(
        ir.contains("__println_num"),
        "expected __println_num in:\n{ir}"
    );
}

#[test]
fn print_string_lowers_to_cstr_intrinsic() {
    let ir = gen_ir(
        r#"
        ^ = () -> Num => <
            print("hello")
            0
        >
    "#,
    );
    assert!(
        ir.contains("__print_cstr"),
        "expected __print_cstr in:\n{ir}"
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
