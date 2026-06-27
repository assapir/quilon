//! Native ahead-of-time build: a type-checked Quilon program -> native executable.
//!
//! Emits an object file directly from the in-process LLVM module via inkwell's
//! `TargetMachine` (so no external `llc` is needed), then links it against the
//! `libquilon_rt` static library + Boehm GC using the system C toolchain
//! (`clang` by default, or `gcc`). Backs the `quilon build` subcommand and
//! supersedes the old `scripts/aot.sh`.

use std::path::{Path, PathBuf};
use std::process::Command;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use crate::ast::Program;
use crate::codegen::CodeGenerator;

/// Emit a native object file for `program` at `obj_path` using LLVM's
/// `TargetMachine`. Uses PIC relocation so string/data relocations link cleanly
/// into a (default) PIE executable.
fn emit_object(program: &Program, obj_path: &Path) -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {e}"))?;

    let context = Context::create();
    // Build the generator with the type oracle installed (precise composite read types).
    let mut generator = CodeGenerator::with_oracle(&context, "main", program)?;
    // Populates, verifies, and builds the C `main` wrapper around `^`.
    generator.generate(program)?;
    let module = generator.module();

    let triple = TargetMachine::get_default_triple();
    let target =
        Target::from_triple(&triple).map_err(|e| format!("Failed to look up target: {e}"))?;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let machine = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            OptimizationLevel::None,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "Failed to create target machine".to_string())?;

    machine
        .write_to_file(module, FileType::Object, obj_path)
        .map_err(|e| format!("Failed to emit object file: {e}"))
}

/// Directory holding the running `quilon` binary, where `libquilon_rt.a` is built
/// alongside it (e.g. `target/debug`). That's the `-L` path the linker needs.
fn runtime_lib_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate quilon binary: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "quilon binary has no parent directory".to_string())?
        .to_path_buf();
    if !dir.join("libquilon_rt.a").exists() {
        return Err(format!(
            "libquilon_rt.a not found next to the quilon binary ({}). Build it with `cargo build`.",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Build `program` into a native executable at `out`, linking with `linker`
/// (`clang` or `gcc`) against `libquilon_rt` + Boehm GC.
pub fn build_native(program: &Program, out: &Path, linker: &str) -> Result<(), String> {
    let obj = out.with_extension("o");
    emit_object(program, &obj)?;
    let lib_dir = runtime_lib_dir()?;

    let status = Command::new(linker)
        .arg(&obj)
        .arg("-L")
        .arg(&lib_dir)
        // The Rust staticlib needs these system libs alongside Boehm GC.
        .args(["-lquilon_rt", "-lgc", "-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(out)
        .status()
        .map_err(|e| format!("failed to invoke linker `{linker}`: {e}"));

    // Drop the intermediate object whether or not linking succeeded.
    let _ = std::fs::remove_file(&obj);

    match status? {
        s if s.success() => Ok(()),
        s => Err(format!("linker `{linker}` failed with {s}")),
    }
}
