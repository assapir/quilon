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
use quilon::diagnostic::codes::Code;
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

/// The `file:line:column:` position line a report prints for `path` — with the path
/// elided exactly as the report elides it.
///
/// A report shortens a path wider than `MAX_PATH_WIDTH` from its start, so an expectation
/// built from the raw path only holds where the temp directory is short. Linux's `/tmp/...`
/// always is; macOS's `/var/folders/<random>/T/...` never is, which is where an expectation
/// spelled with `path.display()` fails while the compiler is behaving exactly as documented.
pub fn position(path: &Path, line: usize, column: usize) -> String {
    let shown = quilon::source_map::shorten_path(&path.display().to_string());
    format!("╭─[{shown}:{line}:{column}]")
}

/// The frame a report draws for a failure at `line`/`column` of `source_line`, underlining
/// `width` characters of it — everything under the `error[…]: message` line. `position`
/// is the report's own position line (see [`position`]).
pub fn frame(
    position: &str,
    line: usize,
    column: usize,
    source_line: &str,
    width: usize,
) -> String {
    let gutter = " ".repeat(line.to_string().len() + 2);
    format!(
        "{gutter}{position}\n {line} │ {source_line}\n{gutter}· {}{}\n{gutter}╰────",
        " ".repeat(column - 1),
        "─".repeat(width)
    )
}

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

/// [`assert_exit`] under its older name: every program is linked now (see [`front_end`]),
/// so the two are the same behavior. Kept because a test naming it documents that its
/// program leans on the corelib (`<< core.io`, `<< core.test`, an implicit `core.text`).
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

/// Assert the type checker rejects `src` with exactly `code`.
pub fn assert_type_error_code(src: &str, code: Code) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let error = TypeChecker::new()
        .check_program(&program)
        .expect_err(&format!("expected a type error for source:\n{src}"));
    assert_eq!(error.code(), code, "wrong code for source:\n{src}\n{error}");
}

/// Assert `src` is REJECTED by the parser (a syntactic error, before type checking).
pub fn assert_parse_error(src: &str) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    assert!(
        parser::parse(&tokens).is_err(),
        "expected a parse error for source:\n{src}"
    );
}

/// Compile and run `src` as a SUBPROCESS (`quilon run`), returning `(exit code, stderr)`
/// and the path it was written to.
///
/// A program that fails loudly calls `__exit`, which would terminate the test runner if it
/// ran in-process — so any test asserting on a failure's output has to spawn. `tag` names
/// the file, which matters because the location the program reports IS that path.
pub fn run_program(tag: &str, src: &str) -> (i32, String, std::path::PathBuf) {
    let run = run_program_named(
        &format!("{tag}{}", quilon::source_extension::EXTENSION),
        src,
    );
    (run.code, run.stderr, run.path)
}

/// What a spawned program did.
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Where the source was written — the path the program reports itself under.
    pub path: std::path::PathBuf,
}

/// Like [`run_program`], but the caller names the file (extension included) and gets the
/// program's stdout as well. Writing `src` under a chosen name is what lets a test say
/// something about the name itself — that a deprecated extension still runs, say.
pub fn run_program_named(file_name: &str, src: &str) -> Run {
    let seq = SUBPROCESS_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("quilon_run_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join(file_name);
    std::fs::write(&file, src).expect("write temp program");
    run_file(&file)
}

/// `quilon run` an existing file, capturing what it produced.
pub fn run_file(file: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().expect("a UTF-8 path")])
        .output()
        .expect("spawn quilon run");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        path: file.to_path_buf(),
    }
}

/// Build `src` into a native executable with `quilon build` and run it, returning
/// `(exit code, stdout)`. The caller must have checked that a linker is on PATH
/// ([`tool_available`]); `tag` names the program's file and its binary.
pub fn build_and_run_native(tag: &str, src: &str) -> (i32, String) {
    let quilon = std::path::PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    ensure_runtime_lib(quilon.parent().expect("the compiler's directory"));

    let seq = SUBPROCESS_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("quilon_build_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join(format!("{tag}.qn"));
    std::fs::write(&file, src).expect("write temp program");
    let binary = dir.join(tag);

    let build = Command::new(&quilon)
        .arg("build")
        .arg(&file)
        .args(["-o".as_ref(), binary.as_os_str()])
        .output()
        .expect("spawn quilon build");
    assert!(
        build.status.success(),
        "quilon build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&binary)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the built executable");
    (
        run.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&run.stdout).into_owned(),
    )
}

/// Serializes nothing — it only keeps concurrently-running tests from colliding on a
/// temp-directory name (a single test binary runs its own tests in parallel).
static SUBPROCESS_SEQ: AtomicU64 = AtomicU64::new(0);

/// Whether `tool` is on PATH, for gates that need a linker and skip gracefully without one.
pub fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// The type error `src` is rejected with, as its report — the coded header with the
/// message, and the `help:` line where the error has one (no source snippet: the program
/// is in memory). Panics if the program type-checks — a test asserting on a diagnostic
/// needs there to be one.
pub fn type_error_message(src: &str) -> String {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let program = quilon::modules::link(program, Path::new("."), None)
        .expect("import linking failed")
        .0;
    match TypeChecker::new().check_program(&program) {
        Err(e) => e.diagnostic().render(&SourceMap::default(), false),
        Ok(_) => panic!("expected a type error for source:\n{src}"),
    }
}

/// Lex, parse, optionally resolve imports, and type-check — panicking with the stage
/// that failed. Returns the program, the type table its check produced, the deferral
/// coloring, and the source map codegen fills call-site `Site` values from. The in-memory
/// source is named [`TEST_FILE`], so a located runtime message (a failed assertion) is
/// reproducible in a test's expected output.
pub fn front_end(
    src: &str,
    base_dir: Option<&Path>,
) -> (quilon::ast::Program, TypeTable, DeferInfo, Rc<SourceMap>) {
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    // Linking is not optional: it is also what resolves qualified references
    // (`io.print` -> `core.io.print`) and merges `core.text` behind a composable Text
    // method call, even in a program importing nothing.
    let dir = base_dir.unwrap_or_else(|| Path::new("."));
    let (program, mut sources) =
        quilon::modules::link(program, dir, None).expect("import linking failed");
    sources.set_root(TEST_FILE, src);
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let defer = deferral::analyze(&program);
    (program, types, defer, Rc::new(sources))
}

/// The path an in-memory test program is reported under — what a failing assertion in a
/// test source prints as its `file` (`test.qn:3:5: ...`).
pub const TEST_FILE: &str = "test.qn";

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
