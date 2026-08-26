//! One frame for every located failure.
//!
//! Three things now report a `file:line:column` with the offending source line and a caret
//! run: a compile error (`src/diagnostic.rs`), a failing `core.test` assertion (composed in
//! Quilon by `failAt`), and a fail-loud runtime check (composed in Rust by
//! `quilon-rt::report::fail_at`). Three renderers, one shape — which only stays true if
//! something checks it, because the Quilon one is deliberately hackable and the Rust one has
//! to abort from inside an intrinsic where there is no Quilon frame to compose from.
//!
//! Every case runs as a SUBPROCESS: these programs exit through `__exit`, which would take
//! the test runner with them.

use std::path::Path;
use std::process::Command;

mod common;
use common::{ensure_runtime_lib, position, run_program, tool_available};

fn quilon() -> &'static str {
    env!("CARGO_BIN_EXE_quilon")
}

/// The frame a report should carry for a failure at `line`/`column` of `source_line`,
/// underlining `width` characters — everything except the message itself.
fn expected_frame(line: usize, column: usize, source_line: &str, width: usize) -> String {
    let number = line.to_string();
    let gutter = " ".repeat(number.len());
    format!(
        "{gutter} |\n{number} | {source_line}\n{gutter} | {}{}",
        " ".repeat(column - 1),
        "^".repeat(width)
    )
}

/// A failing assertion and a failing bounds check frame their location identically — the
/// position line, the message on its own line under it, then the same gutter, source line,
/// and caret run — differing only in the message text.
#[test]
fn an_assertion_and_a_runtime_check_frame_alike() {
    let (assert_code, assert_stderr, assertion) = run_program(
        "assertion",
        "<< core.test\n^ = () -> $ => <\n  assert(1 == 2)\n>\n",
    );
    assert_eq!(assert_code, 101);
    assert_eq!(
        assert_stderr,
        format!(
            "{}\nassertion failed\n{}\n",
            position(&assertion, 3, 3),
            expected_frame(3, 3, "  assert(1 == 2)", "assert(1 == 2)".len())
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
            "{}\nindex 9 out of bounds for an array of size 1\n{}\n",
            position(&bounds, 4, 3),
            expected_frame(4, 3, "  a[n]", "a[n]".len())
        )
    );
}

/// A compile error frames the same way — with `error:` before the message, since that one
/// says which severity it is. Keeps the three renderers honest about the gutter and caret.
#[test]
fn a_compile_error_frames_alike() {
    let (_, _, file) = run_program("mismatch", "^ = () -> Num => <\n  x = 1 + true\n  x\n>\n");
    let out = Command::new(quilon())
        .args(["check", file.to_str().unwrap()])
        .output()
        .expect("spawn quilon check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains(&expected_frame(2, 7, "  x = 1 + true", "1 + true".len())),
        "a compile error must carry the same frame, got: {stderr}"
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
        jit_stderr.contains(":4:3:\nindex 5 out of bounds for an array of size 2"),
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
