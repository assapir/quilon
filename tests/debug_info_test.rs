//! DWARF line-number debug-info test for `quilon build --debug`.
//!
//! Builds a small `.ql` program with `--debug` and shells out to `llvm-dwarfdump` to
//! assert the emitted binary carries a DWARF compile unit that references the `.ql`
//! source: a `.debug_line` file table naming the `.ql` file, and a `.debug_info`
//! subprogram for the user's function decl-lined at its source line. Skips gracefully
//! when the C toolchain or `llvm-dwarfdump` is unavailable (mirrors the native-AOT tests).

use std::path::Path;
use std::process::Command;

/// Is a tool available on PATH (responds to `--version`)?
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Build a FRESH `libquilon_rt.a` next to the `quilon` binary so `quilon build` links it.
/// Mirrors the native-AOT tests' runtime-lib setup.
fn ensure_runtime_lib(bin_dir: &Path) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let rt_target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("rt-staticlib");
    let status = Command::new(&cargo)
        .args(["build", "-p", "quilon-rt"])
        .arg("--target-dir")
        .arg(&rt_target)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status();
    assert!(
        status.is_ok_and(|s| s.success()),
        "failed to build libquilon_rt.a for the debug-info test"
    );
    let fresh = rt_target.join("debug").join("libquilon_rt.a");
    // Copy atomically: other test binaries run concurrently and copy the SAME archive to
    // the SAME destination, so a plain `fs::copy` could interleave into a partial file that
    // a racing `quilon build` then links. Write a process-unique temp in the dest dir and
    // rename over it — the rename is atomic, so every reader sees a complete archive.
    let dest = bin_dir.join("libquilon_rt.a");
    // Unique per call (PID alone is shared by this binary's parallel tests, so add a global
    // counter) so two concurrent copies never target the same temp file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let uniq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = bin_dir.join(format!(
        "libquilon_rt.a.{}.{}.tmp",
        std::process::id(),
        uniq
    ));
    std::fs::copy(&fresh, &tmp).expect("copy fresh libquilon_rt.a to a temp file");
    std::fs::rename(&tmp, &dest).expect("atomically place libquilon_rt.a next to the binary");
}

#[test]
fn debug_build_emits_dwarf_line_info_for_the_ql_source() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    // Need a linker to produce the binary and `llvm-dwarfdump` to inspect it.
    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping debug-info test: need a linker (`clang` or `gcc`) on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping debug-info test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    // A single-file program (no imports) so every emitted function maps to THIS file.
    // `factorial` is on line 2; the entry point `^` is on line 3.
    let src = "\nfactorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)\n^ = () -> Num => factorial(5)\n";
    let dir = std::env::temp_dir().join(format!("quilon_dbg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("prog.ql");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("prog");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["--debug", "-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build --debug");
    assert!(
        build.status.success(),
        "`quilon build --debug --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // The program's exit code is `factorial(5)` == 120 — debug info must not change behavior.
    let run = Command::new(&bin).status().expect("run built binary");
    assert_eq!(
        run.code(),
        Some(120),
        "debug build changed program behavior"
    );

    // `.debug_line`: the line-number program must name the `.ql` source file.
    let line = Command::new("llvm-dwarfdump")
        .arg("--debug-line")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-line");
    assert!(line.status.success(), "llvm-dwarfdump --debug-line failed");
    let line_out = String::from_utf8_lossy(&line.stdout);
    assert!(
        line_out.contains("prog.ql"),
        "expected the `.ql` file in the DWARF line table, got:\n{line_out}"
    );

    // `.debug_info`: a subprogram for `factorial`, attributed to the `.ql` file at line 2.
    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    assert!(info.status.success(), "llvm-dwarfdump --debug-info failed");
    let info_out = String::from_utf8_lossy(&info.stdout);
    assert!(
        info_out.contains("DW_TAG_subprogram"),
        "expected at least one subprogram in the DWARF info, got:\n{info_out}"
    );
    assert!(
        info_out.contains("prog.ql"),
        "expected the `.ql` file referenced by a subprogram's DW_AT_decl_file"
    );
    assert!(
        info_out.contains("\"factorial\""),
        "expected a `factorial` subprogram in the DWARF info"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_debug_build_has_no_ql_debug_info() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping non-debug-info test: need a linker on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping non-debug-info test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let src = "^ = () -> Num => 7\n";
    let dir = std::env::temp_dir().join(format!("quilon_nodbg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("plain.ql");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("plain");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // Without `--debug`, no compile unit should reference the `.ql` source. (The Rust
    // runtime's own debug info may be present, but it never names a `.ql` file.)
    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    let info_out = String::from_utf8_lossy(&info.stdout);
    assert!(
        !info_out.contains("plain.ql"),
        "a non-debug build must not carry `.ql` debug info, got:\n{info_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
