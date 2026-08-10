//! Regression gate for issue #38: the *documented* build flow —
//! `cargo build --release` then `quilon build …` — must work as written, with no
//! extra command and no test-harness fixup.
//!
//! The JIT/AOT parity gate in `examples_test.rs` masks this bug: it builds a
//! fresh `libquilon_rt.a` and copies it next to the binary itself before running
//! `quilon build`. This file deliberately does NOT do that — it relies solely on
//! what the crate's cargo build script (`/build.rs`) places, which is exactly what
//! a user gets from a plain `cargo build`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Is a tool available on PATH? (Used to skip the link step gracefully when no C
/// toolchain is installed — matching `examples_test.rs`.)
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// The build script MUST bake `QUILON_RT_LIB` and place the archive there. This
/// assertion can only pass if the deterministic-placement mechanism ran — it is
/// independent of the parity gate's copy step, so it catches exactly the gap in
/// issue #38 (documented flow broken while parity gate is green).
#[test]
fn build_script_bakes_and_places_runtime_staticlib() {
    let Some(baked) = option_env!("QUILON_RT_LIB") else {
        panic!("build script must bake QUILON_RT_LIB (issue #38 deterministic placement)");
    };
    let path = Path::new(baked);
    assert!(
        path.exists(),
        "baked QUILON_RT_LIB points at a missing file: {baked}"
    );
    assert_eq!(
        path.file_name().and_then(|n| n.to_str()),
        Some("libquilon_rt.a"),
        "baked runtime archive has the wrong name: {baked}"
    );
}

/// The first linker (`clang` preferred, then `gcc`) available on PATH, or
/// `None` — callers skip gracefully.
fn available_linker() -> Option<&'static str> {
    ["clang", "gcc"].into_iter().find(|t| tool_available(t))
}

/// `quilon build examples/hello_world.ql -o out --linker <linker>` with
/// `configure` applied to the command (env tweaks etc.), asserting the build
/// succeeds; then run the produced binary and return its exit code
/// (hello_world's documented code is 42).
fn build_hello_and_run(
    quilon: &Path,
    linker: &str,
    out: &Path,
    context: &str,
    configure: impl FnOnce(&mut Command),
) -> Option<i32> {
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello_world.ql");
    let mut cmd = Command::new(quilon);
    cmd.args(["build", example.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["-o", out.to_str().unwrap()]);
    configure(&mut cmd);
    let build = cmd.output().expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed ({context}): {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(out).output().expect("run produced binary");
    run.status.code()
}

/// End-to-end: run `quilon build` on a real example WITHOUT copying the archive
/// first, and assert the produced native binary runs to its documented exit code.
/// This is the two-command README flow, exercised as written.
#[test]
fn documented_build_flow_produces_running_binary() {
    let Some(linker) = available_linker() else {
        eprintln!("skipping documented-build-flow gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = Path::new(env!("CARGO_BIN_EXE_quilon"));
    let out: PathBuf =
        std::env::temp_dir().join(format!("quilon_issue38_hello_{}", std::process::id()));

    let code = build_hello_and_run(quilon, linker, &out, "documented flow regressed", |_| {});
    let _ = std::fs::remove_file(&out);
    assert_eq!(
        code,
        Some(42),
        "hello_world native binary produced the wrong exit code"
    );
}

/// Distributed-binary scenario: a user downloads ONLY the `quilon` binary — no
/// `libquilon_rt.a` next to it, no build tree on disk. `quilon build` must still
/// work, by decompressing the runtime archive embedded in the binary itself into
/// the per-user cache and linking from there — and a second build must REUSE the
/// cached extraction rather than rewrite it.
///
/// Simulated by copying the built binary into an empty temp dir and running it
/// from there with `QUILON_RT_LIB` cleared (no override) and `XDG_CACHE_HOME`
/// pointed at a fresh temp dir (so the test observes exactly what gets cached,
/// and never touches the real user cache).
#[test]
fn distributed_binary_builds_via_embedded_runtime() {
    let Some(linker) = available_linker() else {
        eprintln!("skipping distributed-binary gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let stage = std::env::temp_dir().join(format!("quilon_dist_sim_{}", std::process::id()));
    let bin_dir = stage.join("bin"); // holds ONLY the copied quilon binary
    let cache = stage.join("cache"); // stands in for $XDG_CACHE_HOME
    std::fs::create_dir_all(&bin_dir).expect("create staged bin dir");

    let quilon = bin_dir.join("quilon");
    std::fs::copy(env!("CARGO_BIN_EXE_quilon"), &quilon).expect("copy quilon binary");
    let out = stage.join("hello");

    let staged_env = |cmd: &mut Command| {
        cmd.env_remove("QUILON_RT_LIB")
            .env("XDG_CACHE_HOME", &cache);
    };

    // Cold cache: the build must succeed by extracting the embedded archive.
    let code = build_hello_and_run(&quilon, linker, &out, "cold cache", staged_env);
    assert_eq!(
        code,
        Some(42),
        "distributed-binary build produced the wrong exit code (cold cache)"
    );

    // The embedded archive must have been extracted to <XDG_CACHE_HOME>/quilon
    // as a content-keyed libquilon_rt-<key>.a, with no leftover temp files.
    let quilon_cache = cache.join("quilon");
    let cached_archive = || -> PathBuf {
        let entries: Vec<PathBuf> = std::fs::read_dir(&quilon_cache)
            .expect("cache dir was created")
            .map(|e| e.expect("read cache entry").path())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one extracted archive in the cache, found: {entries:?}"
        );
        let name = entries[0].file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("libquilon_rt-") && name.ends_with(".a"),
            "cache entry has an unexpected name: {name}"
        );
        entries[0].clone()
    };
    let extracted = cached_archive();
    let cold_mtime = extracted
        .metadata()
        .expect("stat cached archive")
        .modified();

    // Warm cache: a second build must succeed AND reuse the extracted copy
    // (same single file, unchanged mtime — no re-decompression/rewrite).
    let code = build_hello_and_run(&quilon, linker, &out, "warm cache", staged_env);
    assert_eq!(
        code,
        Some(42),
        "distributed-binary build produced the wrong exit code (warm cache)"
    );
    assert_eq!(cached_archive(), extracted, "cache file changed identity");
    let warm_mtime = extracted
        .metadata()
        .expect("stat cached archive")
        .modified();
    assert_eq!(
        cold_mtime.expect("mtime"),
        warm_mtime.expect("mtime"),
        "warm-cache build rewrote the cached archive instead of reusing it"
    );

    let _ = std::fs::remove_dir_all(&stage);
}
