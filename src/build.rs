//! Native ahead-of-time build: a type-checked Quilon program -> native executable.
//!
//! Emits an object file directly from the in-process LLVM module via inkwell's
//! `TargetMachine` (so no external `llc` is needed), then links it against the
//! `libquilon_rt` static library — which carries the Boehm GC — using the system C
//! toolchain (`clang` by default, or `gcc`). Backs the `quilon build` subcommand
//! and supersedes the old `scripts/aot.sh`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use crate::ast::Program;
use crate::codegen::CodeGenerator;
use crate::source_map::SourceMap;
use crate::status::{Stage, Status};
use crate::typechecker::TypeTable;
use std::rc::Rc;

/// What DWARF line-number emission needs beyond the source map: the root `.qn` file's path as
/// the user named it (recorded in the DWARF `DIFile`).
///
/// The file's TEXT is not here — it comes from the `SourceMap` the build already carries,
/// which is the one place a file's path and contents live.
pub struct DebugSource<'a> {
    pub file: &'a Path,
}

/// Emit a native object file for `program` at `obj_path` using LLVM's
/// `TargetMachine`. Uses PIC relocation so string/data relocations link cleanly
/// into a (default) PIE executable. When `debug` is `Some`, DWARF line-number info
/// is emitted (the `--debug` build mode) and the object is left unoptimized — a
/// debugger needs to see every local and step every line, which the optimizer would
/// otherwise inline, reorder, or eliminate. Otherwise the build is optimized: LLVM's
/// O3 pass pipeline (inlining, `mem2reg`, LICM, loop optimizations, …) runs over the
/// module before it is emitted, alongside `OptimizationLevel::Aggressive` backend
/// codegen — the target-machine level alone only tunes instruction selection, not
/// the IR-level middle-end passes that do the actual optimization work.
fn emit_object(
    program: &Program,
    types: TypeTable,
    defer: crate::deferral::DeferInfo,
    sources: Rc<SourceMap>,
    obj_path: &Path,
    debug: Option<&DebugSource<'_>>,
) -> Result<(), String> {
    let optimize = debug.is_none();
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {e}"))?;

    let context = Context::create();
    // The type oracle comes from the front end's check (precise composite read types).
    let mut generator = CodeGenerator::new(&context, "main");
    generator.set_type_table(types);
    generator.set_defer_info(defer);
    generator.set_aot();
    generator.set_source_map(Rc::clone(&sources));
    // Turn on DWARF line-number emission before codegen so every function/expression is
    // attributed to its `.qn` source location.
    if let Some(d) = debug {
        generator.enable_debug(d.file, &sources);
    }
    // Populates, verifies, and builds the C `main` wrapper around `^`.
    generator.generate(program)?;
    let module = generator.module();

    let triple = TargetMachine::get_default_triple();
    let target =
        Target::from_triple(&triple).map_err(|e| format!("Failed to look up target: {e}"))?;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let optimization = if optimize {
        OptimizationLevel::Aggressive
    } else {
        OptimizationLevel::None
    };
    let machine = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            optimization,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "Failed to create target machine".to_string())?;

    // The target machine's own optimization level only tunes backend codegen (instruction
    // selection, scheduling). The actual O3 work — inlining, mem2reg, LICM, loop
    // optimizations — is the middle-end pass pipeline, run explicitly here.
    if optimize {
        module
            .run_passes("default<O3>", &machine, PassBuilderOptions::create())
            .map_err(|e| format!("Failed to run the O3 optimization pipeline: {e}"))?;
    }

    machine
        .write_to_file(module, FileType::Object, obj_path)
        .map_err(|e| format!("Failed to emit object file: {e}"))
}

/// The `libquilon_rt.a` bytes, gzip-compressed and embedded into the compiler
/// binary at compile time. The crate's cargo build script (`/build.rs`) builds
/// the `quilon-rt` staticlib and bakes the compressed copy's path into
/// `QUILON_RT_GZ`; embedding the archive makes a *distributed* `quilon` binary
/// self-contained — `quilon build` works from a bare binary download, with no
/// archive shipped alongside it, and since the archive carries the statically
/// built Boehm GC, so is every binary it produces. Decompressed at most once per
/// compiler version per machine:
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
/// else the temp-dir fallback. Shared with the language server (`quilon/corelibDir`, in
/// `src/lsp.rs`), which materializes the embedded corelib under a subdirectory of it —
/// rather than duplicating the resolution logic.
pub(crate) fn cache_dir() -> PathBuf {
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

/// The process-local portion of a temporary object-stage name. Together with
/// the PID and atomic directory creation, this also keeps builds from separate
/// threads from sharing an intermediate object.
static OBJECT_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Run one build with its object file in an owned temporary directory. The
/// directory reservation is atomic, so each build gets its own `program.o`, and
/// removal happens whether emission, runtime extraction, or linking fails.
fn with_staged_object(build: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let root = temp_cache_dir();
    std::fs::create_dir_all(&root).map_err(|e| {
        format!(
            "failed to create temporary object directory {}: {e}",
            root.display()
        )
    })?;

    let stage = loop {
        let sequence = OBJECT_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = root.join(format!(".build-object-{}-{sequence}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "failed to create temporary object directory {}: {e}",
                    candidate.display()
                ));
            }
        }
    };

    let result = build(&stage.join("program.o"));
    let cleanup = std::fs::remove_dir_all(&stage).map_err(|e| {
        format!(
            "failed to remove temporary object directory {}: {e}",
            stage.display()
        )
    });

    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(build), Err(cleanup)) => Err(format!("{build}; additionally, {cleanup}")),
    }
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

/// System libraries the Rust staticlib needs. The Boehm GC is not among them:
/// `quilon-rt`'s build script compiles it statically, so it is already inside the
/// runtime archive — which is what makes a produced binary runnable on a machine
/// with no libgc installed. Apple's libSystem provides `dlopen` and friends, so
/// there is no `-ldl` to ask for there (and asking is a hard error).
#[cfg(target_os = "macos")]
pub const SYSTEM_LIBS: &[&str] = &["-lpthread", "-lm"];
#[cfg(not(target_os = "macos"))]
pub const SYSTEM_LIBS: &[&str] = &["-lpthread", "-ldl", "-lm"];

/// Drop every section nothing reaches. Both halves of the runtime archive are already split
/// per function and per datum — rustc and the `cc` crate that builds bdwgc each default to
/// that — so this resolves per FUNCTION what the archive scan below can only resolve per
/// object file: a stripped hello-world goes from 1.4 MB to 662 KB. The concurrency runtime
/// stays, being reachable from the `-u` roots; what goes is the std and compiler-support code
/// around it. ld64 spells it `-dead_strip`.
///
/// The object codegen emits is NOT split (LLVM does not default to it and inkwell exposes no
/// setting), which costs nothing: it holds only functions `ast::reachability` already kept,
/// and is kilobytes against the archive's megabytes.
#[cfg(target_os = "macos")]
pub const DEAD_STRIP_ARGS: &[&str] = &["-Xlinker", "-dead_strip"];
#[cfg(not(target_os = "macos"))]
pub const DEAD_STRIP_ARGS: &[&str] = &["-Xlinker", "--gc-sections"];

/// Append the arguments that link `libquilon_rt.a` (`rt_lib`) into the executable, retaining
/// the runtime intrinsics — `#[no_mangle]` symbols nothing in Rust calls, referenced only by
/// the emitted LLVM IR — that a plain archive scan could otherwise drop (nondeterministically,
/// depending on the staticlib's codegen-unit split and archive member order), surfacing as an
/// `undefined reference`.
///
/// On GNU ld the retention is narrow: one `-u <symbol>` per intrinsic seeds an undefined
/// reference the archive scan resolves, pinning exactly those members and pulling the rest in on
/// demand — leaving out the unreferenced compiler-support and std objects that force-including
/// the whole archive drags along, roughly a tenth of a hello-world's size. Driven from
/// `quilon_rt::INTRINSICS`, so a new intrinsic needs no change here.
///
/// It pulls whole objects, and rustc partitions the staticlib into codegen units freely: the
/// object defining `__exit` also defines the scheduler and the `mio` reactor, so a program
/// that never blocks still pulls both. [`DEAD_STRIP_ARGS`] removes the unreached code
/// afterwards.
///
/// ld64 (macOS) force-loads the whole archive instead (`force_load`): a per-intrinsic `-u` would
/// need each name spelled with the Mach-O leading underscore, and ld64 makes an unmatched `-u` a
/// hard link error, so force-loading is both correct and unconditionally safe there.
///
/// The archive is passed by path (not `-L`/`-l`) either way: the cache-extracted copy carries a
/// content-hash suffix in its filename, which `-l` name lookup could not address. The force-load
/// flag and that path are separate `-Xlinker` arguments rather than one `-Wl,` list, because the
/// driver splits a `-Wl,` argument on commas and the archive path is user-controlled — a comma in
/// it would arrive at the linker as two mangled flags.
fn append_runtime_link_args(command: &mut Command, rt_lib: &Path, force_load: bool) {
    if force_load {
        command.arg("-Xlinker").arg("-force_load");
        command.arg("-Xlinker").arg(rt_lib);
    } else {
        for (name, _) in quilon_rt::INTRINSICS {
            command.arg("-u").arg(name);
        }
        command.arg(rt_lib);
    }
}

/// Build `program` into a native executable at `out`, linking with `linker`
/// (`clang` or `gcc`) against `libquilon_rt`, which carries the Boehm GC. The two stages
/// — code generation, then the link — are announced through `status`. Optimized (LLVM O3)
/// unless `debug` is `Some` (see [`emit_object`]).
#[allow(clippy::too_many_arguments)] // one call site, the CLI, which passes what it was given
pub fn build_native(
    program: &Program,
    types: TypeTable,
    defer: crate::deferral::DeferInfo,
    sources: Rc<SourceMap>,
    out: &Path,
    linker: &str,
    debug: Option<&DebugSource<'_>>,
    status: &Status,
) -> Result<(), String> {
    with_staged_object(|obj| {
        status.stage(Stage::Generating);
        emit_object(program, types, defer, sources, obj, debug)?;
        let rt_lib = runtime_lib_path()?;

        status.stage(Stage::Linking);
        let mut command = Command::new(linker);
        command.arg(obj);
        append_runtime_link_args(&mut command, &rt_lib, cfg!(target_os = "macos"));
        command.args(SYSTEM_LIBS);
        command.args(DEAD_STRIP_ARGS);
        command.arg("-o").arg(out);

        let status = command.status().map_err(|e| match e.kind() {
            // Name the missing binary instead of a bare "No such file or
            // directory (os error 2)".
            std::io::ErrorKind::NotFound => format!(
                "linker `{linker}` not found on PATH. Install it, or pass \
                 `--linker <name>` (e.g. `--linker gcc`)."
            ),
            _ => format!("failed to invoke linker `{linker}`: {e}"),
        });

        match status? {
            s if s.success() => Ok(()),
            s => Err(format!("linker `{linker}` failed with {s}")),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_objects_are_unique_and_cleaned_after_a_failed_build() {
        let mut objects = Vec::new();
        let result = with_staged_object(|first| {
            objects.push(first.to_path_buf());
            std::fs::write(first, b"first object").map_err(|e| e.to_string())?;

            with_staged_object(|second| {
                objects.push(second.to_path_buf());
                std::fs::write(second, b"second object").map_err(|e| e.to_string())?;
                Err("link failed".to_string())
            })
        });

        assert_eq!(result, Err("link failed".to_string()));
        assert_ne!(objects[0], objects[1], "builds must use distinct objects");
        assert!(
            objects.iter().all(|object| !object.exists()),
            "failed builds must remove their staged objects: {objects:?}"
        );
    }

    /// A comma in the archive path is enough to mangle a `-Wl,` flag, and the path comes from
    /// the user's cache location, so neither retention shape may build one. Both are checked
    /// from any host: the ld64 form is unreachable on Linux, where it would otherwise go
    /// untested until someone with a comma in `$HOME` ran `quilon build` on a Mac.
    #[test]
    fn neither_retention_shape_passes_a_comma_bearing_path_through_wl() {
        let archive = Path::new("/home/a,b/.cache/quilon/libquilon_rt.a");
        for force_load in [true, false] {
            let mut command = Command::new("cc");
            append_runtime_link_args(&mut command, archive, force_load);
            let args: Vec<String> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();

            assert!(
                !args.iter().any(|arg| arg.starts_with("-Wl,")),
                "a `-Wl,` argument is comma-split by the driver: {args:?}"
            );
            assert!(
                args.iter().any(|arg| arg == archive.to_str().unwrap()),
                "the archive path must reach the linker whole: {args:?}"
            );
        }
    }

    /// ld64 wants `-force_load <archive>` as two arguments, and `-Xlinker` is what keeps them
    /// adjacent and unparsed.
    #[test]
    fn the_force_load_shape_is_two_xlinker_pairs() {
        let archive = Path::new("/tmp/libquilon_rt.a");
        let mut command = Command::new("cc");
        append_runtime_link_args(&mut command, archive, true);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["-Xlinker", "-force_load", "-Xlinker", "/tmp/libquilon_rt.a"]
        );
    }

    /// The GNU-ld shape: one `-u` per intrinsic, all of them before the archive, since a
    /// member is only pulled in for a reference the scan has already seen.
    #[test]
    fn the_undefined_symbol_shape_names_every_intrinsic_before_the_archive() {
        let archive = Path::new("/tmp/libquilon_rt.a");
        let mut command = Command::new("cc");
        append_runtime_link_args(&mut command, archive, false);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args.len(), quilon_rt::INTRINSICS.len() * 2 + 1);
        assert_eq!(args.last().unwrap(), "/tmp/libquilon_rt.a");
        for (name, _) in quilon_rt::INTRINSICS {
            let at = args
                .iter()
                .position(|arg| arg == name)
                .unwrap_or_else(|| panic!("{name} is not forced onto the link"));
            assert_eq!(args[at - 1], "-u");
        }
    }
}
