//! The harness every integration test drives the compiler through.
//!
//! Each `tests/*.rs` file is its own binary, so this module is compiled into each one
//! separately — including `JIT_LOCK`, which therefore stays exactly what it was when
//! each file declared its own: a lock over the JIT within one test binary.
//!
//! That same per-binary compilation is why the whole module allows dead code: a binary
//! that only needs `assert_exit` still compiles `assert_type_error` and the linking
//! variants, and would warn about every helper it happens not to call. The allowance is
//! about how shared test modules build, not about keeping unused code around — every
//! helper here has callers, just never all of them in one binary.
#![allow(dead_code)]

use quilon::deferral::{self, DeferInfo};
use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::source_map::SourceMap;
use quilon::typechecker::{TypeChecker, TypeTable};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// LLVM's JIT and native-target initialization are not safe to run from several threads
/// at once, and cargo runs a binary's tests in parallel — so every execution below is
/// serialized through this.
pub static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Compile and run `src`, asserting the entry point yields `expected` as the exit code.
/// The front end must succeed: a program that fails to lex, parse, or type-check is a
/// broken test, not a passing one.
pub fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (program, types, defer, sources) = front_end(src, None);
    let code = jit::run_program(&program, types, defer, sources, &["program".to_string()])
        .expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{src}");
}

/// Like [`assert_exit`], but resolves `<<` imports first, so a program that uses the
/// core library (`<< core.io`, `<< core.test`, …) runs end to end.
pub fn assert_exit_linked(src: &str, expected: i32) {
    assert_exit_linked_from(src, Path::new("."), expected);
}

/// Like [`assert_exit_linked`], resolving file-path imports relative to `base_dir`.
pub fn assert_exit_linked_from(src: &str, base_dir: &Path, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (program, types, defer, sources) = front_end(src, Some(base_dir));
    let code = jit::run_program(&program, types, defer, sources, &["program".to_string()])
        .expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{src}");
}

/// Assert the type checker REJECTS `src`. Lexing and parsing must still succeed: the
/// point is that the checker caught it, so a source that dies earlier would pass this
/// for the wrong reason.
pub fn assert_type_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    assert!(
        TypeChecker::new().check_program(&program).is_err(),
        "expected a type error for source:\n{src}"
    );
}

/// The type error `src` is rejected with, as its rendered message. Panics if the program
/// type-checks — a test asserting on a diagnostic needs there to be one.
pub fn type_error_message(src: &str) -> String {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let program = quilon::modules::link(program, Path::new("."))
        .expect("import linking failed")
        .0;
    match TypeChecker::new().check_program(&program) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected a type error for source:\n{src}"),
    }
}

/// Lex, parse, optionally resolve imports, and type-check — panicking with the stage
/// that failed. Returns the program, the type table its check produced, the deferral
/// coloring, and the source map codegen fills call-site `Site` values from. The in-memory
/// source is named [`TEST_FILE`], so a located runtime message (a failed assertion) is
/// reproducible in a test's expected output.
fn front_end(
    src: &str,
    base_dir: Option<&Path>,
) -> (quilon::ast::Program, TypeTable, DeferInfo, Rc<SourceMap>) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let (program, mut sources) = match base_dir {
        Some(dir) => quilon::modules::link(program, dir).expect("import linking failed"),
        None => (program, SourceMap::default()),
    };
    sources.set_root(TEST_FILE, src);
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let defer = deferral::analyze(&program);
    (program, types, defer, Rc::new(sources))
}

/// The path an in-memory test program is reported under — what a failing assertion in a
/// test source prints as its `file` (`test.ql:3:5: ...`).
pub const TEST_FILE: &str = "test.ql";

/// An empty source map, for a test that builds its program by hand and does not care where
/// a call site resolves to. A call site then reports the documented "unknown" location
/// (empty file, position 1:1) instead of a real one.
pub fn no_sources() -> Rc<SourceMap> {
    Rc::new(SourceMap::default())
}

/// Put a freshly built `libquilon_rt.a` next to the compiler binary, where `quilon build`
/// looks for it before falling back to the embedded copy.
///
/// **The placement must be atomic.** Test binaries run concurrently, several of them want
/// this archive, and they all want it at the same path — so a plain copy truncates the
/// file that a sibling's linker is reading, which surfaces as `undefined reference to`
/// some intrinsic and looks exactly like a dead-stripping bug. (It was mistaken for one.)
/// Writing a unique temp file in the destination directory and renaming over the target
/// means every reader sees either the old archive or the new one, never a partial one.
///
/// The temp name needs more than the process id: a single test binary runs its own tests
/// in parallel, so two copies from the SAME process must not collide either.
pub fn ensure_runtime_lib(bin_dir: &Path) {
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
        "failed to build libquilon_rt.a for the native-AOT tests"
    );

    let fresh = rt_target.join("debug").join("libquilon_rt.a");
    let dest = bin_dir.join("libquilon_rt.a");
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = bin_dir.join(format!(
        "libquilon_rt.a.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(&fresh, &tmp).expect("copy fresh libquilon_rt.a to a temp file");
    std::fs::rename(&tmp, &dest).expect("atomically place libquilon_rt.a next to the binary");
}
