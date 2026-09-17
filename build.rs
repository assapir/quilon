//! Cargo build script for the `quilon` crate.
//!
//! NOTE: this is the *cargo build script* (runs at `cargo build` time) — not `src/build.rs`,
//! which implements the `quilon build` subcommand (native AOT of a `.qn` program).
//!
//! Two jobs:
//!
//! 1. Libgc link trigger — links libgc via the `#[link(name = "gc")]` extern block on the
//!    actual `GC_malloc`/`GC_init` symbol references in `src/runtime/intrinsics.rs`, rather
//!    than a bare `cargo:rustc-link-lib=gc` here, which `--as-needed` can drop. libgc must
//!    be installed (`libgc-dev` on Debian/Ubuntu, `gc` on Arch); CI installs it.
//!
//! 2. Deterministically place the runtime staticlib — cargo only uplifts a dependency's
//!    staticlib to `target/<profile>/` when that crate is a primary build target, so as a
//!    mere dependency `quilon-rt`'s archive lands at `target/<profile>/deps/` under a
//!    hashed name `quilon build` can't find. This job builds `quilon-rt` itself into an
//!    isolated `--target-dir` (so it doesn't re-enter this script or deadlock on the outer
//!    build's `target/` lock) and copies the result to the fixed name
//!    `libquilon_rt.bundled.a` (baked as `QUILON_RT_LIB`) next to the `quilon` binary, and
//!    embeds a gzip-compressed copy (`QUILON_RT_GZ`, keyed by `QUILON_RT_KEY`) that
//!    `src/build.rs` `include_bytes!`s into the compiler binary, so a distributed `quilon`
//!    binary (no archive alongside it) can extract and link the runtime from its own
//!    embedded copy.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support/deployment_target.rs");

    // Rebuild (and re-place) the staticlib whenever the runtime crate — or a
    // dependency it pins — changes, so a stale `libquilon_rt.a` can never linger
    // next to the binary.
    println!("cargo:rerun-if-changed=quilon-rt/src");
    println!("cargo:rerun-if-changed=quilon-rt/Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    place_runtime_staticlib();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!(
            "cargo:rustc-env=QUILON_MACOS_DEPLOYMENT_TARGET={}",
            macos_deployment_target()
        );
    }
}

// Shared with `quilon-rt/build.rs`, which needs the same lookup for the GC object.
include!("build_support/deployment_target.rs");

/// Build the `quilon-rt` staticlib and copy it to
/// `target/<profile>/libquilon_rt.bundled.a` (next to the `quilon` binary), then
/// bake that path into the binary as `QUILON_RT_LIB`.
fn place_runtime_staticlib() {
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));

    // OUT_DIR is `<target>/<profile>/build/<pkg>-<hash>/out`; four levels up is
    // `<target>/<profile>`, the directory the `quilon` binary is uplifted into
    // (holds for cross builds too: `<target>/<triple>/<profile>`).
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the expected layout")
        .to_path_buf();

    let profile = env("PROFILE"); // "debug" or "release"
    let is_release = profile == "release";

    // Isolated target dir for the nested build: keeps its own build cache so the
    // staticlib is emitted deterministically, and never contends for the outer
    // build's `target/` lock.
    let nested_target = out_dir.join("rt-staticlib");

    let cargo = env("CARGO");
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .args(["-p", "quilon-rt"])
        .arg("--target-dir")
        .arg(&nested_target)
        .current_dir(&manifest_dir);
    if is_release {
        cmd.arg("--release");
    }

    // Always pass an explicit `--target` (the triple cargo already selected for
    // us via `TARGET`). This pins the nested build's output layout to
    // `<nested>/<triple>/<profile>/` deterministically — an explicit `--target`
    // flag overrides any inherited `CARGO_BUILD_TARGET` env or `.cargo/config.toml`
    // `build.target`, either of which would otherwise silently move the artifact
    // under a triple subdir and desync it from where we look. For a native build
    // `TARGET` is just the host triple, so this is a no-op beyond the extra path
    // component (which we account for below).
    let target = env("TARGET");
    cmd.args(["--target", &target]);
    let produced = nested_target
        .join(&target)
        .join(if is_release { "release" } else { "debug" })
        .join("libquilon_rt.a");

    let status = cmd
        .status()
        .expect("failed to spawn `cargo build -p quilon-rt`");
    assert!(
        status.success(),
        "nested `cargo build -p quilon-rt` failed with {status}"
    );
    assert!(
        produced.exists(),
        "quilon-rt staticlib not found at {}",
        produced.display()
    );

    let dest = profile_dir.join("libquilon_rt.bundled.a");
    std::fs::copy(&produced, &dest)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", produced.display(), dest.display()));

    // Bake the path so the copy next to the binary keeps serving the dev loop
    // (`quilon build` looks there before touching the embedded copy).
    println!("cargo:rustc-env=QUILON_RT_LIB={}", dest.display());

    // Embed support: `src/build.rs` `include_bytes!`s a *gzip-compressed* copy of
    // the archive so a distributed binary is self-contained without carrying the
    // full uncompressed staticlib in its image. Also bake a content key for the
    // (uncompressed) archive (64-bit FNV-1a), so `quilon build` can name its
    // cache-extracted copy without rehashing the blob on every invocation. Cargo
    // reruns this script (and rustc re-embeds) whenever the archive can change,
    // so key and bytes stay in sync.
    let bytes = std::fs::read(&dest).unwrap_or_else(|e| panic!("read {}: {e}", dest.display()));
    let key = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    println!("cargo:rustc-env=QUILON_RT_KEY={key:016x}");

    let gz_path = out_dir.join("libquilon_rt.a.gz");
    let gz_file = std::fs::File::create(&gz_path)
        .unwrap_or_else(|e| panic!("create {}: {e}", gz_path.display()));
    let mut encoder = flate2::write::GzEncoder::new(gz_file, flate2::Compression::best());
    std::io::Write::write_all(&mut encoder, &bytes)
        .and_then(|()| encoder.finish().map(drop))
        .unwrap_or_else(|e| panic!("compress {}: {e}", gz_path.display()));
    println!("cargo:rustc-env=QUILON_RT_GZ={}", gz_path.display());
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} not set in build script environment"))
}
