//! Native ahead-of-time build: a type-checked Quilon program -> native executable.
//!
//! Emits an object file directly from the in-process LLVM module via inkwell's
//! `TargetMachine` (so no external `llc` is needed), then links it against the
//! `libquilon_rt` static library + Boehm GC using the system C toolchain
//! (`clang` by default, or `gcc`). Backs the `quilon build` subcommand and
//! supersedes the old `scripts/aot.sh`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use crate::ast::Program;
use crate::codegen::CodeGenerator;

/// The source needed to emit DWARF line-number debug info: the `.ql` file's path (recorded
/// in the DWARF `DIFile`) and its text (to map span byte offsets to `(line, column)`).
pub struct DebugSource<'a> {
    pub file: &'a Path,
    pub source: &'a str,
    /// How many leading program items came from imported modules (import linking prepends
    /// them); those get no debug info, as their spans are relative to their own module source.
    pub imported_items: usize,
}

/// Emit a native object file for `program` at `obj_path` using LLVM's
/// `TargetMachine`. Uses PIC relocation so string/data relocations link cleanly
/// into a (default) PIE executable. When `debug` is `Some`, DWARF line-number info
/// is emitted (the `--debug` build mode); otherwise the object carries no debug info.
fn emit_object(
    program: &Program,
    obj_path: &Path,
    debug: Option<&DebugSource<'_>>,
) -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {e}"))?;

    let context = Context::create();
    // Build the generator with the type oracle installed (precise composite read types).
    let mut generator = CodeGenerator::with_oracle(&context, "main", program)?;
    // Turn on DWARF line-number emission before codegen so every function/expression is
    // attributed to its `.ql` source location.
    if let Some(d) = debug {
        generator.enable_debug(d.file, d.source, d.imported_items);
    }
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

/// The `libquilon_rt.a` bytes, gzip-compressed and embedded into the compiler
/// binary at compile time. The crate's cargo build script (`/build.rs`) builds
/// the `quilon-rt` staticlib and bakes the compressed copy's path into
/// `QUILON_RT_GZ`; embedding the archive makes a *distributed* `quilon` binary
/// self-contained — `quilon build` works from a bare binary download, with no
/// archive shipped alongside it (system libgc is still required, as documented
/// in the README). Decompressed at most once per compiler version per machine:
/// only when no system-provided archive exists and the cache misses.
const QUILON_RT_ARCHIVE_GZ: &[u8] = include_bytes!(env!("QUILON_RT_GZ"));

/// Content key of the embedded archive (64-bit FNV-1a, hashed by the cargo
/// build script and baked in alongside the bytes) — names the extracted cache
/// file so a new compiler never links a stale archive left by an older one,
/// without rehashing the multi-megabyte blob on every `quilon build`.
const QUILON_RT_KEY: &str = env!("QUILON_RT_KEY");

/// The value of an environment variable, with empty treated as unset.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Per-user cache directory: `$XDG_CACHE_HOME/quilon`, else `~/.cache/quilon`,
/// else the temp-dir fallback.
fn cache_dir() -> PathBuf {
    if let Some(xdg) = env_non_empty("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("quilon");
    }
    if let Some(home) = env_non_empty("HOME") {
        return Path::new(&home).join(".cache").join("quilon");
    }
    temp_cache_dir()
}

/// Last-resort cache location, also used when the per-user cache is unwritable.
fn temp_cache_dir() -> PathBuf {
    std::env::temp_dir().join("quilon-cache")
}

/// Extract the embedded runtime archive into `dir` as `libquilon_rt-<key>.a`.
/// If the keyed file already exists it is already this exact content — reuse it
/// with no decompression. Otherwise decompress the embedded gzip bytes and
/// write atomically: a process-unique temp file in the same directory, then a
/// rename over the destination, so concurrent `quilon build` processes can race
/// here without ever exposing a partially written archive.
fn extract_archive_into(dir: &Path) -> std::io::Result<PathBuf> {
    let dest = dir.join(format!("libquilon_rt-{QUILON_RT_KEY}.a"));
    if dest.exists() {
        return Ok(dest);
    }

    let mut archive = Vec::new();
    flate2::read::GzDecoder::new(QUILON_RT_ARCHIVE_GZ).read_to_end(&mut archive)?;

    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".libquilon_rt-{QUILON_RT_KEY}.{}.tmp",
        std::process::id()
    ));
    std::fs::write(&tmp, &archive)?;
    std::fs::rename(&tmp, &dest).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(dest)
}

/// The path of the `libquilon_rt.a` archive (the `quilon-rt` staticlib) to link.
/// Resolved, in order:
///
/// 1. `QUILON_RT_LIB` set in the *runtime* environment — developer override.
/// 2. `libquilon_rt.a` next to the running binary — the dev loop: the cargo
///    build script places it there, and the test harness drops fresh ones in.
/// 3. The per-user cache: an already-extracted copy keyed to this compiler's
///    archive is reused as-is; only on a miss is the embedded (gzip) archive
///    decompressed into it — the always-works path for a distributed binary.
fn runtime_lib_path() -> Result<PathBuf, String> {
    // 1. Explicit runtime override.
    if let Some(over) = env_non_empty("QUILON_RT_LIB") {
        let over = PathBuf::from(over);
        return if over.exists() {
            Ok(over)
        } else {
            Err(format!(
                "QUILON_RT_LIB is set but points at a missing file: {}",
                over.display()
            ))
        };
    }

    // 2. Next to the running binary (cheap; skips the extraction entirely).
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let local = dir.join("libquilon_rt.a");
        if local.exists() {
            return Ok(local);
        }
    }

    // 3. Extract the embedded archive to the cache (temp dir on cache failure).
    let cache = cache_dir();
    extract_archive_into(&cache)
        .or_else(|e| {
            let fallback = temp_cache_dir();
            if fallback == cache {
                Err(e) // cache_dir() already bottomed out at the temp dir
            } else {
                extract_archive_into(&fallback)
            }
        })
        .map_err(|e| format!("failed to extract the embedded runtime archive: {e}"))
}

/// Build `program` into a native executable at `out`, linking with `linker`
/// (`clang` or `gcc`) against `libquilon_rt` + Boehm GC.
pub fn build_native(
    program: &Program,
    out: &Path,
    linker: &str,
    debug: Option<&DebugSource<'_>>,
) -> Result<(), String> {
    let obj = out.with_extension("o");
    emit_object(program, &obj, debug)?;
    let rt_lib = runtime_lib_path()?;

    let status = Command::new(linker)
        .arg(&obj)
        // Pull EVERY object out of `libquilon_rt.a`, not just the members that resolve
        // an already-undefined symbol. The Rust staticlib splits the `#[no_mangle]`
        // runtime intrinsics across codegen-unit objects (and their order in the archive
        // is unspecified), so a single linker pass over the archive can miss an
        // intrinsic the program references (e.g. `__text_cmp`) when its defining object
        // sits earlier than the object that first pulled the archive in — manifesting as
        // an `undefined reference` only under whatever CU split CI happens to produce.
        // `--whole-archive` makes inclusion deterministic; `--no-whole-archive` restores
        // normal (on-demand) linking for the system libs that follow. The archive is
        // passed by path (not `-L`/`-l`): the cache-extracted copy carries a content-hash
        // suffix in its filename, which `-l` name lookup couldn't address.
        .arg("-Wl,--whole-archive")
        .arg(&rt_lib)
        .arg("-Wl,--no-whole-archive")
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
