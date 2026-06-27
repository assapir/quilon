//! Native-AOT tests for the `^` entry point receiving `args :: []Text` and
//! `env :: [][]Text`. Unlike the JIT (which threads in this test process's own
//! argv/environment), a freshly built native binary lets us pass an EXPLICIT argv
//! and environment and assert that `args.size`, `args[i]`, and the `[key, value]`
//! env pairs reflect exactly what we passed. Skips gracefully if no C toolchain
//! (`clang`/`gcc` + the `quilon` binary's runtime lib) is available.

use std::path::Path;
use std::process::Command;

/// Is a tool available on PATH?
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Build a FRESH `libquilon_rt.a` next to the `quilon` binary (`quilon build` links it
/// from there). Mirrors `examples_test::ensure_runtime_lib`: a dedicated target dir
/// forces a fresh staticlib emit so a newly added runtime intrinsic (`__argv_to_text_array`,
/// `__envp_to_pairs`) is present for AOT linking.
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
        "failed to build libquilon_rt.a for the native args test"
    );
    let fresh = rt_target.join("debug").join("libquilon_rt.a");
    std::fs::copy(&fresh, bin_dir.join("libquilon_rt.a"))
        .expect("copy fresh libquilon_rt.a next to the quilon binary");
}

/// Compile `src` to a native binary at `out` via `quilon build`, returning whether a
/// linker was available (false -> the caller should skip).
fn build_native(quilon: &str, src: &str, out: &Path) -> bool {
    let linker = ["clang", "gcc"].into_iter().find(|t| tool_available(t));
    let Some(linker) = linker else {
        eprintln!("skipping native args test: need a linker (`clang` or `gcc`) on PATH");
        return false;
    };
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let tmp_src = out.with_extension("ql");
    std::fs::write(&tmp_src, src).expect("write temp source");
    let build = Command::new(quilon)
        .args(["build", tmp_src.to_str().unwrap(), "--linker", linker])
        .args(["-o", out.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    true
}

#[test]
fn native_args_size_reflects_passed_argv() {
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let bin = std::env::temp_dir().join(format!("quilon_args_size_{}", std::process::id()));
    if !build_native(quilon, "^ = (args :: []Text) -> Num => args.size", &bin) {
        return;
    }

    // argv[0] is the program path itself, so `args.size` == 1 + (extra args passed).
    let none = Command::new(&bin).output().expect("run native");
    assert_eq!(
        none.status.code(),
        Some(1),
        "no extra args -> args.size == 1"
    );

    let three = Command::new(&bin)
        .args(["a", "b", "c"])
        .output()
        .expect("run native");
    assert_eq!(
        three.status.code(),
        Some(4),
        "3 extra args -> args.size == 4 (incl. argv[0])"
    );

    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(bin.with_extension("ql"));
}

#[test]
fn native_env_pairs_split_on_first_equals() {
    // The env is `[][]Text` of `[key, value]` pairs split on the FIRST `=`. Print the
    // first pair's key and value, then exit on the env size. Run with a controlled env
    // (`env -i` would be ideal but isn't portable; instead pass a known var and count).
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let bin = std::env::temp_dir().join(format!("quilon_env_pairs_{}", std::process::id()));
    let src = "<< core.io\n\
               ^ = (args :: []Text, env :: [][]Text) -> Num => <\n\
               \x20 pair = env[0]\n\
               \x20 print(pair[0])\n\
               \x20 print(pair[1])\n\
               \x20 env.size\n\
               >";
    if !build_native(quilon, src, &bin) {
        return;
    }

    // A single env var with a value containing '=' proves the split is on the FIRST '='.
    let out = Command::new(&bin)
        .env_clear()
        .env("KEY", "a=b=c")
        .output()
        .expect("run native");
    assert_eq!(
        out.status.code(),
        Some(1),
        "exactly one env var -> env.size == 1"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout, "KEY\na=b=c\n",
        "first pair should be [KEY, a=b=c] (split on the FIRST '=')"
    );

    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(bin.with_extension("ql"));
}
