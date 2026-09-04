use super::*;
use crate::lexer::Lexer;
use crate::parser::parse;
use crate::typechecker::{TypeChecker, TypeTable};

/// Type-check `code`, then generate it with the checker's type-oracle wired in — the
/// path every real compilation takes. Codegen reads a function/block's type from the
/// oracle rather than re-deriving it, so a test exercising that (return types, a block
/// ending in a declaration) must run the checker first, exactly like `quilon run` does.
fn generate_checked(code: &str) -> Result<String, String> {
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();
    let types = TypeChecker::new()
        .check_program(&program)
        .unwrap_or_else(|e| panic!("type check failed: {:?}", e));
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");
    codegen.set_type_table(types);
    codegen.generate(&program)
}

/// A hand-built oracle entry for every `pattern` substring's byte range in `code`, typed
/// `[]Num` — for a test that asks codegen a question WITHOUT running the type checker
/// (so a checker rejection elsewhere in the program cannot pre-empt what codegen itself
/// is being asked), but still needs an array literal's element type recorded (codegen
/// treats a missing oracle entry for an array literal as a compiler bug, never a guess).
fn num_array_oracle(code: &str, pattern: &str) -> TypeTable {
    let element_type = Type::Array(Box::new(Type::Num));
    code.match_indices(pattern)
        .map(|(start, matched)| {
            let start = start as u32;
            let span = Span::in_root(start, start + matched.len() as u32);
            (span, element_type.clone())
        })
        .collect()
}

#[test]
fn test_simple_number() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let tokens = Lexer::tokenize("x = 42").unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    // Global variable with float value
    assert!(ir.contains("4.2") || ir.contains("42"));
}

#[test]
fn test_simple_function() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => < a + b >").unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    assert!(result.is_ok());

    let ir = result.unwrap();
    assert!(ir.contains("define"));
    assert!(ir.contains("add"));
}

#[test]
fn test_local_variable() {
    let code = "double = x :: Num => < y = x + x y >";
    let result = generate_checked(code);
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Local variable
    assert!(ir.contains("load")); // Variable load
    assert!(ir.contains("store")); // Variable store
    assert!(ir.contains("fadd")); // Addition
}

#[test]
fn test_array() {
    // Test array in a function body - return the first element as a number
    let code = "sum = x :: Num => < arr = [x, x, x] x >";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Array allocation
    assert!(ir.contains("getelementptr")); // Array element access
}

#[test]
fn test_function_call() {
    // Test calling a function
    let code = "
        add = (a :: Num, b :: Num) => < a + b >
        main = => < add(3, 4) >
    ";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("call")); // Function call
    assert!(ir.contains("@add")); // Call to add function
    assert!(ir.contains("fadd")); // Addition in add function
}

#[test]
fn test_record() {
    // Test record creation
    let code = "make_point = (x :: Num, y :: Num) => < p = {x = x, y = y} x >";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Struct allocation
    assert!(ir.contains("getelementptr")); // Field access
}

#[test]
fn test_field_access() {
    // Test field access
    let code = "get_x = (a :: Num, b :: Num) => < p = {x = a, y = b} p.x >";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("getelementptr")); // Field GEP
    assert!(ir.contains("load")); // Field load
}

#[test]
fn test_method_codegen_and_dispatch() {
    // A named record with a method; the entry point constructs an instance and calls it.
    // All fields are Num so the field layout/access is exact.
    let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => < it.x + it.y >
}

^ = () -> Num => <
  p = Point { x = 3, y = 4 }
  p.sum()
>";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    // The method is emitted as a mangled top-level function taking the receiver pointer.
    assert!(ir.contains("@Point_sum"));
    // And the call site dispatches to it.
    assert!(ir.contains("call") && ir.contains("Point_sum"));
}

#[test]
fn test_method_calls_sibling_method() {
    // `doubled` calls the sibling method `sum` via `it.sum()` — exercises the signature
    // pre-pass (forward reference) and `it`-based dispatch.
    let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => < it.x + it.y >,
  doubled = => < it.sum() + it.sum() >
}

^ = () -> Num => <
  p = Point { x = 10, y = 5 }
  p.doubled()
>";
    let result = generate_checked(code);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("@Point_sum"));
    assert!(ir.contains("@Point_doubled"));
}

/// Every symbol the runtime exports must have a prototype here, and every prototype must
/// name a symbol the runtime exports. The two halves used to be kept in step by hand,
/// and a miss did not fail the build — it produced a call to a null address at run time.
/// (`memcpy` is libc's, so it is the one prototype with no runtime counterpart.)
#[test]
fn every_runtime_intrinsic_can_be_declared() {
    let context = Context::create();
    let codegen = CodeGenerator::new(&context, "intrinsic_parity");

    for (name, _) in quilon_rt::INTRINSICS {
        assert!(
            codegen.get_intrinsic(name).is_ok(),
            "the runtime exports `{name}` but codegen has no prototype for it, so a call \
             to it would be emitted against a symbol codegen never declared"
        );
    }

    assert!(
        codegen.get_intrinsic("memcpy").is_ok(),
        "libc's memcpy stays declarable"
    );
    assert!(
        codegen.get_intrinsic("__not_a_real_intrinsic").is_err(),
        "a name the runtime does not export must be refused, not declared"
    );
}

/// `Text` comparison is chosen by the operands' TYPE, never by their LLVM shape. Arrays
/// (and closures, and sums) are `{ .. }` structs too, so a shape-keyed rule would hand an
/// array to `__text_cmp`, which reads field 0 as a byte pointer and field 1 as a byte
/// length. The type checker rejects `[]Num == []Num` before codegen ever sees it; codegen
/// is asked here directly, without that pass, so the routing itself is what's under test.
#[test]
fn comparing_arrays_is_not_lowered_as_a_text_comparison() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let code = "same = () -> Bool => < a = [1, 2] b = [1, 2] a == b >";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();
    codegen.set_type_table(num_array_oracle(code, "[1, 2]"));

    let result = codegen.generate(&program);
    let error = result.expect_err("array comparison has no lowering");
    assert!(
        error.contains("Eq requires"),
        "expected a refusal, not a Text comparison: {error}"
    );
}

/// The mixed pair too: one `Text` operand does not make the comparison a `Text` comparison,
/// or the other operand is the one read as a pointer and a length.
#[test]
fn comparing_a_text_against_an_array_is_not_lowered_as_a_text_comparison() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let code = r#"same = () -> Bool => < a = "x" b = [1, 2] a == b >"#;
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();
    codegen.set_type_table(num_array_oracle(code, "[1, 2]"));

    let error = codegen
        .generate(&program)
        .expect_err("a Text/array comparison has no lowering");
    assert!(
        error.contains("Eq requires"),
        "expected a refusal, not a Text comparison: {error}"
    );
}

/// The same routing, from the other side: two `Text` operands DO reach `__text_cmp`.
#[test]
fn comparing_texts_calls_the_text_compare_intrinsic() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let code = r#"same = () -> Bool => < a = "x" b = "y" a == b >"#;
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let ir = codegen
        .generate(&program)
        .expect("text comparison compiles");
    assert!(ir.contains("@__text_cmp"), "expected __text_cmp in:\n{ir}");
}

/// A sum's payload slot must be sized by the payload's VALUE representation — the shape a
/// payload is actually stored as — not by the standalone lowering of its type. The two
/// differ for a composite payload: an array value is the `{ ptr, i64 }` struct, while the
/// type alone lowers to a bare pointer, so a pointer-sized slot would leave a stored payload
/// overrunning the one beside it. Today's checker admits only payloads where the two
/// coincide, which is exactly why the rule needs pinning here rather than in a `.qn` program.
#[test]
fn a_sum_payload_slot_is_sized_by_the_payloads_value_representation() {
    let context = Context::create();
    let codegen = CodeGenerator::new(&context, "test");

    let slots = codegen
        .payload_slot_types(&[
            variant("Full", vec![Type::Array(Box::new(Type::Num))]),
            variant("Empty", vec![]),
        ])
        .expect("lay out the payload slots");

    assert_eq!(
        slots,
        vec![codegen.ptr_len_struct_type().into()],
        "the array payload's slot should hold the ptr/len value, not a bare pointer"
    );
}

/// A position where one variant is still a type variable and another is concrete takes the
/// CONCRETE type: the type variable's `double` representation is narrower than a `{ptr,i64}`
/// payload, so choosing it would size the slot below what is stored there.
#[test]
fn a_payload_slot_prefers_a_concrete_field_over_a_type_variable() {
    let context = Context::create();
    let codegen = CodeGenerator::new(&context, "test");

    let generic = Type::Generic {
        name: "T".to_string(),
        arguments: vec![],
    };
    let slots = codegen
        .payload_slot_types(&[
            variant("Pending", vec![generic.clone()]),
            variant("Done", vec![Type::Text]),
        ])
        .expect("lay out the payload slots");
    assert_eq!(slots, vec![codegen.ptr_len_struct_type().into()]);

    // With nothing concrete anywhere, the slot holds a type variable's own representation.
    let only_generic = codegen
        .payload_slot_types(&[variant("Pending", vec![generic])])
        .expect("lay out the payload slots");
    assert_eq!(only_generic, vec![context.f64_type().into()]);
}

fn variant(name: &str, fields: Vec<Type>) -> crate::ast::SumVariant {
    crate::ast::SumVariant {
        name: name.to_string(),
        fields,
    }
}
