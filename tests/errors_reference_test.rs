//! `docs/tooling/errors.md` as executable truth: for every registered code, the first
//! `quilon`/`quilon ignore` example fence in its section is run through the real compiler
//! (front end, JIT, or CLI, by family) and must raise that code and no other.
//!
//! An example spanning more than one file adds a further fence per extra file, titled
//! `quilon title="name.qn"` (see `## The codes` in the reference) — [`example_for`] collects
//! those as siblings written next to the root example before it is run.
//!
//! A handful of codes are backstops the current compiler has no reachable path to from real
//! source (a defensive branch the grammar or the checker's own guarantees keep dead) —
//! [`UNVERIFIABLE`] names each with why, so the gap is a visible, deliberate line rather than
//! a silently-skipped fence.

use quilon::diagnostic::codes::{self, ALL, Code};
use quilon::driver;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Codes verified directly against the file system rather than a source fence: their
/// reference section shows the CLI command that hits them, not `.qn` source (the failure is
/// about the file's path, read before a single byte of it is lexed).
const DIRECT_ONLY: &[Code] = &[Code::SourceNotReadable, Code::NotAQuilonSource];

/// Codes checked by spawning the CLI rather than calling `driver::front_end` directly: each
/// is raised outside the shared front end (`src/main.rs`, for a check only `run`/`build`/
/// `compile` make), so a program that front-end-checks clean still needs the real command to
/// observe it.
const CLI_ONLY: &[Code] = &[Code::NoEntryPoint];

/// [`Code::NestingTooDeep`] alone: the reference keeps its example elided
/// (`((((((… 200 levels …))))))`) for a human to read, so this code's own real 200-level
/// source is generated in Rust instead of read from the fence (see
/// `qn101_nesting_too_deep_example_raises_its_own_code`) — and run as the real `quilon`
/// binary, the way `tests/parse_depth_test.rs` runs every one of its deep-nesting cases,
/// since the recursive-descent parser still spends one native stack frame per level before
/// its guard trips, past a `cargo test` thread's smaller default stack.
const SUBPROCESS_ONLY: &[Code] = &[Code::NestingTooDeep];

/// Codes with no path a real program reaches today, each with why — checked against the
/// compiler as it stands, not asserted from the doc prose alone (see the section comment
/// above each reason for where it was verified).
const UNVERIFIABLE: &[(Code, &str)] = &[
    // `parse_match` (src/parser/ast_parser/patterns.rs) is only ever entered right after
    // `Parser::parse_ternary` has just checked the next token IS `|` (exprs.rs) — so its own
    // "arms.is_empty()" branch, guarding a `?` with no `|` at all, can never run.
    (
        Code::EmptyMatch,
        "parse_match is only called when the next token is already `|`, so its own \
         empty-arms branch never executes",
    ),
    // A file import's canonical name is always its short alias (`ast::nodes::binding_name`
    // returns the bare stem), so two imports sharing a short name always collide as QN205
    // before an ambiguity could register; the built-in modules (`core.io` … `core.info`)
    // have no two sharing a last dotted segment either — confirmed against the running
    // compiler: `<< core.http` + `<< "vendor/http.qn"` (the reference's own illustration)
    // raises QN205, not QN207.
    (
        Code::AmbiguousModulePrefix,
        "no combination of imports shares a display alias between two DIFFERENT canonical \
         names today — a file import's canonical is always its alias (any collision there \
         is QN205), and no two built-in modules share a last segment",
    ),
    // Recognizing a top-level call as a test block already requires the literal chain to
    // resolve to `core.test.describe` (`Parser::at_test_block`), which only resolves when
    // `<< core.test` is the import in scope — and that module always defines
    // `reportSummary`.
    (
        Code::NoTestHarness,
        "a recognized test block requires `<< core.test` already resolved, and that module \
         always defines `core.test.reportSummary` — the backstop has no path from a real \
         import",
    ),
    (
        Code::CodegenFailed,
        "the checker and generator agree on every construct the language reference \
         documents, by the project's own account of this code — no known program reaches it",
    ),
    (
        Code::MatchFailed,
        "the checker proves every match exhaustive before codegen sees it — the runtime \
         backstop has no known program that reaches it",
    ),
];

fn docs_manifest_relative(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// One fenced block in a reference section: its info string (trimmed, e.g. `quilon ignore`
/// or `quilon title="a.qn"`) and body.
struct Fence {
    info: String,
    body: String,
}

/// Every fenced block in `section`, in document order.
fn fences_in(section: &str) -> Vec<Fence> {
    let lines: Vec<&str> = section.lines().collect();
    let mut fences = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(info) = lines[i].strip_prefix("```") {
            let info = info.trim().to_string();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            fences.push(Fence {
                info,
                body: body.join("\n"),
            });
        }
        i += 1;
    }
    fences
}

/// A code's reference example: the root source (its first `quilon`/`quilon ignore` fence)
/// and any `quilon title="name"` fences alongside it, each a sibling file the root's imports
/// resolve against.
struct Example {
    source: String,
    siblings: Vec<(String, String)>,
}

/// `code`'s [`Example`], read from its section of `docs/tooling/errors.md` — `None` when the
/// section carries no `quilon`/`quilon ignore` fence (a CLI-command or output-only section).
fn example_for(code: Code) -> Option<Example> {
    let fences = fences_in(codes::explain(code)?);
    let source = fences
        .iter()
        .find(|f| f.info == "quilon" || f.info == "quilon ignore")?
        .body
        .clone();
    let siblings = fences
        .iter()
        .filter_map(|f| {
            let name = f.info.strip_prefix("quilon title=\"")?.strip_suffix('"')?;
            Some((name.to_string(), f.body.clone()))
        })
        .collect();
    Some(Example { source, siblings })
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh temp directory named after `tag`, for one example's sibling files.
fn temp_dir_for(tag: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "quilon_errors_ref_{}_{tag}_{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_siblings(dir: &Path, siblings: &[(String, String)]) {
    for (name, content) in siblings {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create sibling's parent dir");
        }
        std::fs::write(&path, content).expect("write sibling file");
    }
}

/// Run `source` through the real front end (written to `<dir>/root.qn` — on disk, since a
/// multi-file example's sibling may import it back, as an import cycle's does) and return
/// the code it failed with, if any.
///
/// A real `.qn` file ends with a newline; the markdown fence a doc example comes from does
/// not necessarily preserve one on its last line (QN003's example is exactly a string left
/// open at that line — the lexer's own rule is "reaches a newline before the closing quote",
/// not raw end-of-file, see `src/lexer/token.rs`), so one is added here rather than asking
/// the doc to carry a trailing blank line for it.
fn front_end_code(dir: &Path, source: &str) -> Option<Code> {
    let root = dir.join("root.qn");
    let mut source = source.to_string();
    if !source.ends_with('\n') {
        source.push('\n');
    }
    std::fs::write(&root, &source).expect("write root example file");
    driver::front_end(&root).err().map(|e| e.diagnostic.code)
}

/// Every family 0–3 (front-end) code's example, checked against the real front end — this is
/// most of the registry, and every one of these examples follows the same shape: source in,
/// a [`quilon::diagnostic::Code`] out.
#[test]
fn every_front_end_example_raises_its_own_code() {
    let mut failures = Vec::new();
    let mut checked = 0usize;

    for &code in ALL {
        if code.number() >= 400
            || DIRECT_ONLY.contains(&code)
            || CLI_ONLY.contains(&code)
            || SUBPROCESS_ONLY.contains(&code)
        {
            continue;
        }
        if UNVERIFIABLE
            .iter()
            .any(|(unverifiable, _)| *unverifiable == code)
        {
            continue;
        }

        checked += 1;
        let example = example_for(code).unwrap_or_else(|| {
            panic!("{code}: its reference section has no `quilon`/`quilon ignore` example")
        });
        let dir = temp_dir_for(&code.to_string());
        write_siblings(&dir, &example.siblings);
        let got = front_end_code(&dir, &example.source);
        let _ = std::fs::remove_dir_all(&dir);

        if got != Some(code) {
            failures.push(format!(
                "{code} ({}): its example raised {got:?}, not {code}:\n{}",
                code.title(),
                example.source
            ));
        }
    }

    assert!(
        checked > 0,
        "no front-end codes were checked — the gate would pass by iterating nothing"
    );
    assert!(
        failures.is_empty(),
        "{} front-end example(s) do not raise their own code:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Write `source` to a temp `.qn` file of its own (not shared — a runtime failure's process
/// may abort loudly, and a build's output binary needs a stable path beside it) and return
/// its path.
fn write_program(tag: &str, source: &str) -> PathBuf {
    let dir = temp_dir_for(tag);
    let path = dir.join("program.qn");
    std::fs::write(&path, source).expect("write temp program");
    path
}

fn quilon_run(program: &Path, stdin: Stdio) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", program.to_str().expect("a UTF-8 path")])
        .stdin(stdin)
        .output()
        .expect("spawn `quilon run`");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// QN500/QN501/QN502: a fail-loud runtime check reports through the same coded frame a
/// compile error does — `stderr` carries `error[QNxxx]:` literally — so their reference
/// examples are checked the same way the front-end ones are, just run instead of checked.
#[test]
fn runtime_examples_raise_their_own_code() {
    for code in [
        Code::AssertionFailed,
        Code::IndexOutOfBounds,
        Code::RangeEndpointNotWhole,
        Code::ReplaceAllEmptyFrom,
    ] {
        let example = example_for(code)
            .unwrap_or_else(|| panic!("{code}: its reference section has no example"));
        let program = write_program(&code.to_string(), &example.source);
        let (exit, stderr) = quilon_run(&program, Stdio::null());
        assert_ne!(exit, 0, "{code}: the example must fail, got: {stderr}");
        assert!(
            stderr.contains(&format!("error[{code}]")),
            "{code}: stderr must carry its own code, got: {stderr}"
        );
    }
}

/// QN504: the one report the collector cannot afford to build through the coded frame (it
/// has just failed to find memory, so it writes a raw, code-less line rather than risk
/// allocating a `String` — see `quilon-rt/src/mem.rs::out_of_memory`). Matched on that fixed
/// message instead of `error[QN504]:`, which this path never prints.
#[test]
fn qn504_allocation_failed_example_raises_its_own_code() {
    let example =
        example_for(Code::AllocationFailed).expect("QN504's reference section has an example");
    let program = write_program("QN504", &example.source);
    let (exit, stderr) = quilon_run(&program, Stdio::null());
    assert_ne!(exit, 0, "QN504's example must fail, got: {stderr}");
    assert!(
        stderr.contains("out of memory: cannot allocate"),
        "QN504's example must report the collector's fixed out-of-memory line, got: {stderr}"
    );
}

/// QN505: `@readStdin` meets an IO error other than EOF. Opening a directory and handing its
/// fd to the child as stdin reliably produces one (`read` on a directory fd is `EISDIR` on
/// both Linux and macOS) without needing a broken pipe or a closed descriptor. The
/// reference's own section shows the CLI's report, not `.qn` source (an IO error is an
/// environment condition, not something a fenced program can carry), so the program that
/// reaches `@readStdin` is written here instead.
#[test]
fn qn505_read_failed_example_raises_its_own_code() {
    let program = write_program(
        "QN505",
        "<< core.io\n^ = () -> Num => <\n  io.print(@readStdin())\n  0\n>\n",
    );
    let stdin_dir = temp_dir_for("QN505_stdin");
    let directory_as_stdin = std::fs::File::open(&stdin_dir).expect("open a directory for reading");
    let (_exit, stderr) = quilon_run(&program, Stdio::from(directory_as_stdin));
    assert!(
        stderr.contains(&format!("error[{}]", Code::ReadFailed)),
        "QN505's example must report its own code, got: {stderr}"
    );
}

/// QN401: the reference shows the linker's own words, which only a real failed link
/// produces — a program that type-checks and generates fine, built with a linker name that
/// cannot exist.
#[test]
fn qn401_native_build_failure_raises_its_own_code() {
    let program = write_program("QN401", "^ = () -> Num => < 0 >\n");
    let out = program.with_file_name("program_out");
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["build", program.to_str().unwrap(), "--linker"])
        .arg("definitely-not-a-real-linker-xyz")
        .args(["-o", out.to_str().unwrap()])
        .output()
        .expect("spawn `quilon build`");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the build must fail, got: {stderr}"
    );
    assert!(
        stderr.contains(&format!("error[{}]", Code::BuildFailed)),
        "QN401's example must report its own code, got: {stderr}"
    );
}

/// QN101: nested past the parser's depth guard. The reference's own example is elided for
/// a human to read (see [`SUBPROCESS_ONLY`]), so the real nested source is built here —
/// 200 levels, the same margin `tests/parse_depth_test.rs` uses over the 128-level guard —
/// and run as the real binary rather than in-process.
#[test]
fn qn101_nesting_too_deep_example_raises_its_own_code() {
    let n = 200;
    let source = format!("^ = () -> Num => < {}1{} >\n", "(".repeat(n), ")".repeat(n));
    let program = write_program("QN101", &source);
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["check", program.to_str().unwrap()])
        .output()
        .expect("spawn `quilon check`");
    assert_eq!(
        output.status.code(),
        Some(1),
        "the example must fail cleanly, not crash: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("error[{}]", Code::NestingTooDeep)),
        "QN101's example must report its own code, got: {stderr}"
    );
}

/// QN339: `run`/`build`/`compile` reject a file with no `^` — a check `src/main.rs` makes
/// after the shared front end succeeds (`quilon check` does not), so it needs the real
/// command rather than a call to `driver::front_end`, which this example passes cleanly.
#[test]
fn qn339_no_entry_point_example_raises_its_own_code() {
    let example =
        example_for(Code::NoEntryPoint).expect("QN339's reference section has an example");
    let program = write_program("QN339", &example.source);
    let output = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", program.to_str().unwrap()])
        .output()
        .expect("spawn `quilon run`");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the example must fail, got: {stderr}"
    );
    assert!(
        stderr.contains(&format!("error[{}]", Code::NoEntryPoint)),
        "QN339's example must report its own code, got: {stderr}"
    );
}

/// QN000: the file named on the command line does not exist. The reference shows the CLI
/// invocation, not `.qn` source — this is checked directly against the path, not a fence.
#[test]
fn qn000_source_not_readable() {
    let missing = docs_manifest_relative("target/quilon_errors_ref_definitely_missing.qn");
    match driver::front_end(&missing) {
        Ok(_) => panic!("a missing file must fail"),
        Err(error) => assert_eq!(error.diagnostic.code, Code::SourceNotReadable),
    }
}

/// QN001: the file named on the command line has an extension other than `.qn`. Checked
/// before the file is even opened, so a nonexistent path is enough.
#[test]
fn qn001_not_a_quilon_source() {
    let wrong_extension = docs_manifest_relative("target/quilon_errors_ref_program.ql");
    match driver::front_end(&wrong_extension) {
        Ok(_) => panic!("a `.ql` file must fail"),
        Err(error) => assert_eq!(error.diagnostic.code, Code::NotAQuilonSource),
    }
}

/// The ignore list stays a deliberate, visible line: naming a count here means adding or
/// removing an entry shows up as a reviewable diff, not a silent change in what is skipped.
#[test]
fn unverifiable_codes_are_named_and_explained() {
    assert_eq!(
        UNVERIFIABLE.len(),
        5,
        "the unverifiable list changed size — update this count as part of that change"
    );
    for (code, reason) in UNVERIFIABLE {
        assert!(
            !reason.is_empty(),
            "{code} is on the unverifiable list with no reason"
        );
        println!("{code} is not verified against a live example: {reason}");
    }
}

/// Every registered code is checked by exactly one of: the front-end loop above, one of the
/// family 4/5 tests, a direct file-system check, or the unverifiable list — so a future code
/// with no test written for it fails here instead of silently passing by omission.
#[test]
fn every_registered_code_has_a_verification_path() {
    let explicitly_checked = [
        Code::BuildFailed,
        Code::AssertionFailed,
        Code::IndexOutOfBounds,
        Code::RangeEndpointNotWhole,
        Code::AllocationFailed,
        Code::ReadFailed,
        Code::ReplaceAllEmptyFrom,
    ];
    let missing: Vec<String> = ALL
        .iter()
        .filter(|code| {
            !(code.number() < 400
                || explicitly_checked.contains(code)
                || UNVERIFIABLE
                    .iter()
                    .any(|(unverifiable, _)| unverifiable == *code))
        })
        .map(|code| code.to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "these codes have no verification path in this file: {}",
        missing.join(", ")
    );
}
