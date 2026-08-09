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

/// End-to-end: run `quilon build` on a real example WITHOUT copying the archive
/// first, and assert the produced native binary runs and exits 0 (examples are
/// self-asserting). This is the two-command README flow, exercised as written.
#[test]
fn documented_build_flow_produces_running_binary() {
    let linker = ["clang", "gcc"].into_iter().find(|t| tool_available(t));
    let Some(linker) = linker else {
        eprintln!("skipping documented-build-flow gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello_world.ql");

    let out: PathBuf =
        std::env::temp_dir().join(format!("quilon_issue38_hello_{}", std::process::id()));

    let build = Command::new(quilon)
        .args(["build", example.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["-o", out.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed (documented flow regressed): {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&out).output().expect("run produced binary");
    let _ = std::fs::remove_file(&out);
    assert_eq!(
        run.status.code(),
        Some(0),
        "hello_world native binary produced the wrong exit code"
    );
}
