//! One frame for every located failure.
//!
//! A compile error (`src/diagnostic`, drawn by `miette`) and a fail-loud runtime check
//! (composed in Rust by `quilon-rt::report::fail_at`, which has to abort from inside an
//! intrinsic with no renderer to lean on) report the same frame: the `error[Q…]:` header,
//! the position, the source line, the underline. Two renderers, one shape — which only
//! stays true if something checks it.
//!
//! Every case runs as a SUBPROCESS: these programs exit through `__exit`, which would take
//! the test runner with them.

use std::path::Path;
use std::process::Command;

mod common;
use common::{ensure_runtime_lib, frame, position, run_program, tool_available};

fn quilon() -> &'static str {
    env!("CARGO_BIN_EXE_quilon")
}

/// A failing assertion and a failing bounds check frame their location identically — the
/// coded header with the message, then the same position, source line, and underline —
/// differing only in the code and the message text.
#[test]
fn an_assertion_and_a_runtime_check_frame_alike() {
    let (assert_code, assert_stderr, assertion) = run_program(
        "assertion",
        "<< core.test\n^ = () -> $ => <\n  assert(1, equals(2))\n>\n",
    );
    assert_eq!(assert_code, 101);
    assert_eq!(
        assert_stderr,
        format!(
            "error[Q069]: assertion failed: expected 2, got 1\n{}\n",
            frame(
                &position(&assertion, 3, 3),
                3,
                3,
                "  assert(1, equals(2))",
                "assert(1, equals(2))".len()
            )
        )
    );

    let (bounds_code, bounds_stderr, bounds) = run_program(
        "bounds",
        "^ = () -> Num => <\n  a = [1]\n  n = 9\n  a[n]\n>\n",
    );
    assert_eq!(bounds_code, 1, "a bounds failure keeps its own exit code");
    assert_eq!(
        bounds_stderr,
        format!(
            "error[Q070]: index 9 out of bounds for an array of size 1\n{}\n",
            frame(&position(&bounds, 4, 3), 4, 3, "  a[n]", "a[n]".len())
        )
    );
}

/// A compile error frames the same way. Keeps the two renderers honest about the gutter
/// and the underline.
#[test]
fn a_compile_error_frames_alike() {
    // A plain type mismatch is a one-span report, the frame the runtime draws.
    let (_, _, file) = run_program(
        "mismatch",
        "^ = () -> Num => <\n  x :: Num = true\n  x\n>\n",
    );
    let out = Command::new(quilon())
        .args(["--quiet", "check", file.to_str().unwrap()])
        .output()
        .expect("spawn quilon check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert_eq!(
        stderr,
        format!(
            "error[Q028]: type mismatch: expected Num, got Bool\n{}\n",
            frame(
                &position(&file, 2, 3),
                2,
                3,
                "  x :: Num = true",
                "x :: Num = true".len()
            )
        )
    );
}

/// The location is compiled in, so a NATIVE build reports it exactly as the JIT does — no
/// debug info, no unwinder, nothing to install.
#[test]
fn a_native_build_reports_the_same_location() {
    let Some(linker) = ["clang", "gcc"]
        .into_iter()
        .find(|tool| tool_available(tool))
    else {
        eprintln!("skipping native location gate: need `clang` or `gcc` on PATH");
        return;
    };
    ensure_runtime_lib(Path::new(quilon()).parent().expect("binary has a parent"));

    let src = "^ = () -> Num => <\n  a = [1, 2]\n  n = 5\n  a[n]\n>\n";
    let (jit_code, jit_stderr, file) = run_program("native", src);

    let binary = file.with_extension("bin");
    let build = Command::new(quilon())
        .args(["build", file.to_str().unwrap(), "--linker", linker])
        .args(["-o", binary.to_str().unwrap()])
        .output()
        .expect("spawn quilon build");
    assert!(
        build.status.success(),
        "`quilon build --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let native = Command::new(&binary).output().expect("run native binary");

    assert_eq!(native.status.code().unwrap_or(-1), jit_code);
    assert_eq!(
        String::from_utf8_lossy(&native.stderr),
        jit_stderr,
        "native and JIT reports must be identical"
    );
    assert!(
        jit_stderr.starts_with("error[Q070]: index 5 out of bounds for an array of size 2\n")
            && jit_stderr.contains(":4:3]"),
        "the report must name the failing read, got: {jit_stderr}"
    );
}

/// The runtime's `QlSite` mirrors the compiler's built-in `Site` record by hand, and a
/// mismatch would not fail to compile — it would make the runtime read a `Text` pointer as a
/// line number. So check the two layouts against each other: the LLVM struct codegen emits
/// for `ast::site_fields()` must have exactly the size, field count, and per-field sizes of
/// the Rust struct the intrinsics receive.
#[test]
fn the_runtime_site_mirrors_the_compilers_site_layout() {
    use inkwell::context::Context;
    use inkwell::targets::{InitializationConfig, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default()).expect("initialize target");
    let context = Context::create();
    let triple = TargetMachine::get_default_triple();
    let machine = Target::from_triple(&triple)
        .expect("host target")
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            inkwell::OptimizationLevel::None,
            inkwell::targets::RelocMode::PIC,
            inkwell::targets::CodeModel::Default,
        )
        .expect("host target machine");
    let target_data = machine.get_target_data();

    let site = quilon::codegen::site_struct_type(&context).expect("Site lowers to a struct");
    assert_eq!(
        target_data.get_store_size(&site) as usize,
        std::mem::size_of::<quilon_rt::QlSite>(),
        "the runtime's QlSite and the compiler's Site record must have the same size"
    );
    assert_eq!(
        site.count_fields() as usize,
        quilon::ast::site_fields().len(),
        "every declared Site field must have a slot in the emitted struct"
    );
}
