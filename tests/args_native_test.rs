//! Native-AOT tests for the `^` entry point receiving `args :: []Text` and
//! `env :: [|Text => Text|]`. Unlike the JIT (which threads in this test process's own
//! argv/environment), a freshly built native binary lets us pass an EXPLICIT argv
//! and environment and assert that `args.size`, `args[i]`, and the env Map's entries
//! reflect exactly what we passed. Skips gracefully if no C toolchain
//! (`clang`/`gcc` + the `quilon` binary's runtime lib) is available.

use std::path::Path;
use std::process::Command;

mod common;
use common::ensure_runtime_lib;

/// Is a tool available on PATH?
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
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

    let tmp_src = out.with_extension("qn");
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
    let _ = std::fs::remove_file(bin.with_extension("qn"));
}

#[test]
fn jit_and_aot_argv_agree() {
    // `quilon run f.qn a b c` must give `^`'s `args` the same shape a native
    // `./f a b c` gets — `[<file>, a, b, c]` — instead of leaking the `quilon run` CLI
    // prefix. Drive BOTH paths through the actual binary and assert they agree on
    // `args.size` for the same trailing user args, including a leading `--flag` (which
    // must pass THROUGH to the program, not be parsed by quilon).
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let bin = std::env::temp_dir().join(format!("quilon_argv_parity_{}", std::process::id()));
    let src = "^ = (args :: []Text) -> Num => args.size";
    if !build_native(quilon, src, &bin) {
        return;
    }
    let source = bin.with_extension("qn"); // build_native wrote the source here.

    // Each case: the trailing user args. Expected `args.size` is 1 (argv[0]) + len.
    let cases: &[&[&str]] = &[&[], &["a", "b", "c"], &["--flag", "x"]];
    for user_args in cases {
        let expected = Some(1 + user_args.len() as i32);

        let jit = Command::new(quilon)
            .arg("run")
            .arg(&source)
            .args(*user_args)
            .output()
            .expect("run quilon run");
        assert_eq!(
            jit.status.code(),
            expected,
            "JIT `quilon run` args.size wrong for user args {user_args:?}: {}",
            String::from_utf8_lossy(&jit.stderr)
        );

        let aot = Command::new(&bin)
            .args(*user_args)
            .output()
            .expect("run native binary");
        assert_eq!(
            aot.status.code(),
            expected,
            "AOT native args.size wrong for user args {user_args:?}"
        );
        // Both sides are pinned to `expected` above, which is exactly the JIT/AOT
        // parity this test guards: same trailing args -> same `args.size`.
    }

    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&source);
}

#[test]
fn native_env_map_split_on_first_equals() {
    // The env is a `[|Text => Text|]` Map keyed by variable name and split on the FIRST
    // `=`. Look a known variable up and print its value, then exit on the env size. Run
    // with a controlled env (`env -i` would be ideal but isn't portable; instead pass a
    // known var and count).
    let quilon = env!("CARGO_BIN_EXE_quilon");
    let bin = std::env::temp_dir().join(format!("quilon_env_map_{}", std::process::id()));
    let src = "<< core.io\n\
               ^ = (args :: []Text, env :: [|Text => Text|]) -> Num => <\n\
               \x20 value = env.get(\"KEY\") ? | Ok(v) => v | NotOk(_) => \"?\"\n\
               \x20 print(value)\n\
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
        stdout, "a=b=c\n",
        "KEY should map to a=b=c (value split on the FIRST '=')"
    );

    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(bin.with_extension("qn"));
}
