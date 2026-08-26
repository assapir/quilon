//! End-to-end proof of the value-returning `@readStdin` primitive and force-on-use.
//!
//! `@readStdin()` launches a background stdin line read and returns a DEFERRED `Text`; the deferred
//! value flows through a binding and is FORCED where a strict primitive reads its bytes (a
//! comparison inside `assertEq`, or a `print`). The standard examples gate can't drive `@readStdin`
//! (it pipes no input), so these tests spawn the compiler as a subprocess with a controlled
//! stdin — proving the read value flowed through and forced correctly on both the in-process
//! JIT (`quilon run`) and, when a linker is present, a native AOT binary (`quilon build`).

mod common;

use common::ensure_runtime_lib;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A program that binds `@readStdin()` to `line` and asserts it equals `"hello"`. The deferred
/// value is forced at the `assertEq` comparison: matching input exits 0, anything else trips
/// the assertion (exit 101) — which is what proves the real read value reached the compare.
const ASSERT_READ: &str = r#"
<< core.io
<< core.test

^ = () -> Num => <
  line = @readStdin()
  assertEq(line, "hello")
  0
>
"#;

/// A program that forces `@readStdin()` directly at a `print` (a strict native output arg) and
/// echoes the line back — proving the deferred value reaches `print` and is forced there.
const ECHO_READ: &str = r#"
<< core.io

^ = () -> Num => <
  print(@readStdin())
  0
>
"#;

/// Two `@readStdin()` launches in one scope. They overlap eagerly but stdin is a single serial
/// stream, so the gate makes them read CONSECUTIVE lines in launch order — `first` then
/// `second`. Proves concurrent reads neither crash (racing fd 0) nor drop/interleave bytes.
const TWO_READS: &str = r#"
<< core.io
<< core.test

^ = () -> Num => <
  first = @readStdin()
  second = @readStdin()
  assertEq(first, "hello")
  assertEq(second, "world")
  0
>
"#;

/// A program that echoes the line it read TWICE — first through `write` (raw bytes, no
/// newline), then through `print` (rendered, plus a newline). Whatever bytes stdin carried,
/// the two output paths are visible side by side in one stdout capture.
const WRITE_THEN_PRINT: &str = r#"
<< core.io

^ = () -> Num => <
  line = @readStdin()
  write(line, stdout)
  print(line)
  0
>
"#;

/// Write `source` to a unique temp `.qn` file and return its path.
fn temp_ql(tag: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_read_{tag}_{}_{}.qn",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, source).expect("write temp .qn");
    path
}

/// Run `command`, feeding `input` to its stdin, and return `(exit code, captured stdout)`.
/// Stdout stays RAW BYTES: the byte-fidelity test below compares output that is deliberately
/// not valid UTF-8, which a lossy decode would erase.
fn run_with_stdin(mut command: Command, input: &[u8]) -> (Option<i32>, Vec<u8>) {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn quilon subprocess");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write to child stdin");
    let output = child
        .wait_with_output()
        .expect("wait for quilon subprocess");
    (output.status.code(), output.stdout)
}

/// `quilon run <file>` (in-process JIT) with `input` piped to stdin.
fn jit_run(file: &Path, input: &[u8]) -> (Option<i32>, Vec<u8>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command.args(["run", file.to_str().unwrap()]);
    run_with_stdin(command, input)
}

/// The first available linker (`clang`, then `gcc`), or `None` to skip the AOT half.
fn available_linker() -> Option<&'static str> {
    ["clang", "gcc"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    })
}

#[test]
fn jit_read_forces_at_a_strict_comparison() {
    let file = temp_ql("assert", ASSERT_READ);

    // Matching input: the deferred value forces to "hello" at the compare → assertion holds.
    let (code, _) = jit_run(&file, b"hello\n");
    assert_eq!(
        code,
        Some(0),
        "@readStdin value should force to \"hello\" and pass"
    );

    // Different input: the SAME forced value must reach the compare and fail the assertion —
    // proving the real read flowed through, not a constant.
    let (code, _) = jit_run(&file, b"goodbye\n");
    assert_eq!(
        code,
        Some(101),
        "a non-matching @readStdin value must trip the assertion"
    );

    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_read_forces_at_a_print() {
    let file = temp_ql("echo", ECHO_READ);
    let (code, stdout) = jit_run(&file, b"transform me\n");
    assert_eq!(code, Some(0));
    assert_eq!(
        stdout, b"transform me\n",
        "print should force the deferred @readStdin value and echo the line"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn jit_two_reads_serialize_into_consecutive_lines() {
    // Two eager @readStdin launches must read consecutive lines in order (the stdin gate
    // serializes them) — not crash on a shared fd or drop/interleave bytes.
    let file = temp_ql("two", TWO_READS);
    let (code, _) = jit_run(&file, b"hello\nworld\n");
    assert_eq!(
        code,
        Some(0),
        "two @readStdin launches should read \"hello\" then \"world\""
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn aot_read_forces_at_a_strict_comparison() {
    let Some(linker) = available_linker() else {
        eprintln!("skipping AOT @readStdin gate: need a linker (`clang` or `gcc`) on PATH");
        return;
    };

    let quilon = env!("CARGO_BIN_EXE_quilon");
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let source = temp_ql("assert_aot", ASSERT_READ);
    let binary = std::env::temp_dir().join(format!("quilon_read_aot_{}", std::process::id()));

    let build = Command::new(quilon)
        .args(["build", source.to_str().unwrap(), "--linker", linker])
        .args(["-o", binary.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let (ok_code, _) = run_with_stdin(Command::new(&binary), b"hello\n");
    assert_eq!(
        ok_code,
        Some(0),
        "native AOT: matching @readStdin should pass"
    );
    let (bad_code, _) = run_with_stdin(Command::new(&binary), b"goodbye\n");
    assert_eq!(
        bad_code,
        Some(101),
        "native AOT: non-matching @readStdin must trip the assertion"
    );

    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&binary);
}

#[test]
fn write_is_byte_verbatim_and_print_renders_the_same_text() {
    // stdin is the one door a Text arrives through unvalidated, so it is where the two
    // output paths can be told apart: `write` emits the bytes as they came, `print` emits
    // the same text rendered for a reader — each invalid byte as U+FFFD — plus a newline.
    // A NUL is content in both: neither path stops at it or shortens the output.
    let file = temp_ql("bytes", WRITE_THEN_PRINT);
    let (code, stdout) = jit_run(&file, b"a\0b\xffc\n");
    assert_eq!(code, Some(0));

    let (verbatim, rendered) = stdout.split_at(5);
    assert_eq!(
        verbatim, b"a\0b\xffc",
        "`write` should pass the bytes through"
    );
    assert_eq!(
        rendered,
        "a\0b\u{fffd}c\n".as_bytes(),
        "`print` should render the invalid byte and keep the NUL"
    );

    let _ = std::fs::remove_file(&file);
}
