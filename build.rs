//! Cargo build script for the `quilon` crate.
//!
//! NOTE: this is the *cargo build script* (runs at `cargo build` time). It is a
//! different file from `src/build.rs`, which implements the `quilon build`
//! subcommand (native AOT of a `.ql` program). Don't confuse the two.
//!
//! Two jobs:
//!
//! 1. libgc link trigger — the Boehm GC (libgc) is linked via a
//!    `#[link(name = "gc")]` extern block in `src/runtime/intrinsics.rs` rather
//!    than here: attaching the link to the actual `GC_malloc`/`GC_init` symbol
//!    references keeps the linker from dropping libgc under `--as-needed` (which
//!    a bare `cargo:rustc-link-lib=gc` here is subject to, depending on link
//!    order). libgc must be installed to build/run Quilon (e.g. `libgc-dev` on
//!    Debian/Ubuntu, `gc` on Arch). CI installs it explicitly.
//!
//! 2. Deterministically place `libquilon_rt.a` (issue #38) — `quilon build`
//!    links the compiled program against the `quilon-rt` *staticlib*. Cargo only
//!    *uplifts* a dependency's staticlib to `target/<profile>/` when that crate is
//!    a primary build target; as a mere dependency of `quilon`, cargo emits it to
//!    `target/<profile>/deps/libquilon_rt-<hash>.a` and never to the canonical
//!    `target/<profile>/libquilon_rt.a`. So `cargo build --release` followed by
//!    `quilon build …` (the documented flow) used to fail: the archive wasn't
//!    where `quilon build` looks for it.
//!
//!    We can't just copy the `deps/` archive from here: this build script runs
//!    *before* cargo compiles the `quilon-rt` dependency, so at this point the
//!    archive doesn't exist yet. Instead we build `quilon-rt` ourselves into an
//!    isolated target dir (a nested `cargo build -p quilon-rt` — the same
//!    technique `tests/examples_test.rs` uses; `-p` means the `quilon` bin/build
//!    script is *not* re-entered, so there is no recursion, and a dedicated
//!    `--target-dir` avoids deadlocking on the outer build's `target/` lock),
//!    then copy the freshly emitted `libquilon_rt.a` to the canonical location
//!    next to where the `quilon` binary lands and bake that path into the binary
//!    via `QUILON_RT_LIB`. `src/build.rs` reads `QUILON_RT_LIB` at runtime.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Rebuild (and re-place) the staticlib whenever the runtime crate — or a
    // dependency it pins — changes, so a stale `libquilon_rt.a` can never linger
    // next to the binary.
    println!("cargo:rerun-if-changed=quilon-rt/src");
    println!("cargo:rerun-if-changed=quilon-rt/Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    place_runtime_staticlib();
}

/// Build the `quilon-rt` staticlib and copy it to the canonical
/// `target/<profile>/libquilon_rt.a` (next to the `quilon` binary), then bake
/// that path into the binary as `QUILON_RT_LIB`.
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

    let dest = profile_dir.join("libquilon_rt.a");
    std::fs::copy(&produced, &dest)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", produced.display(), dest.display()));

    // Bake the canonical path so `src/build.rs` resolves it deterministically at
    // runtime, independent of the current working directory.
    println!("cargo:rustc-env=QUILON_RT_LIB={}", dest.display());
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} not set in build script environment"))
}
