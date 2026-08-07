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

/// The `-L` directory that holds `libquilon_rt.a` (the `quilon-rt` staticlib).
///
/// The crate's cargo build script (`/build.rs`) deterministically places the
/// archive next to the `quilon` binary and bakes its canonical path into
/// `QUILON_RT_LIB` (see that file for why the ordinary build doesn't uplift it —
/// issue #38). We resolve, in order:
///
/// 1. `QUILON_RT_LIB` — the path baked at build time (authoritative).
/// 2. `libquilon_rt.a` next to the running binary — covers relocated binaries and
///    the test harness, which drops a fresh archive there itself.
fn runtime_lib_dir() -> Result<PathBuf, String> {
    // 1. The path baked by the build script, if the archive is still there.
    if let Some(baked) = option_env!("QUILON_RT_LIB") {
        let baked = Path::new(baked);
        if baked.exists()
            && let Some(dir) = baked.parent()
        {
            return Ok(dir.to_path_buf());
        }
    }

    // 2. Fall back to looking next to the running binary.
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate quilon binary: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "quilon binary has no parent directory".to_string())?
        .to_path_buf();
    if dir.join("libquilon_rt.a").exists() {
        return Ok(dir);
    }

    Err(format!(
        "libquilon_rt.a not found next to the quilon binary ({}). \
         Rebuild the compiler with `cargo build --release` (the build script \
         produces and places the runtime archive automatically).",
        dir.display()
    ))
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
        // Pull EVERY object out of `libquilon_rt.a`, not just the members that resolve
        // an already-undefined symbol. The Rust staticlib splits the `#[no_mangle]`
        // runtime intrinsics across codegen-unit objects (and their order in the archive
        // is unspecified), so a single linker pass over a plain `-lquilon_rt` can miss an
        // intrinsic the program references (e.g. `__text_cmp`) when its defining object
        // sits earlier than the object that first pulled the archive in — manifesting as
        // an `undefined reference` only under whatever CU split CI happens to produce.
        // `--whole-archive` makes inclusion deterministic; `--no-whole-archive` restores
        // normal (on-demand) linking for the system libs that follow.
        .args([
            "-Wl,--whole-archive",
            "-lquilon_rt",
            "-Wl,--no-whole-archive",
        ])
        // System libs the Rust staticlib needs, alongside Boehm GC.
        .args(["-lgc", "-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(out)
        .status()
        .map_err(|e| match e.kind() {
            // Name the missing binary instead of a bare "No such file or
            // directory (os error 2)" (issue #38 bonus).
            std::io::ErrorKind::NotFound => format!(
                "linker `{linker}` not found on PATH. Install it, or pass \
                 `--linker <name>` (e.g. `--linker gcc`)."
            ),
            _ => format!("failed to invoke linker `{linker}`: {e}"),
        });

    // Drop the intermediate object whether or not linking succeeded.
    let _ = std::fs::remove_file(&obj);

    match status? {
        s if s.success() => Ok(()),
        s => Err(format!("linker `{linker}` failed with {s}")),
    }
}
