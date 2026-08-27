//! Doc-samples gate: guarantees every ```quilon fence under `docs/` stays
//! compilable, the way `examples_test.rs` guards `examples/`. Each fence must
//! pass the shared front-end (`quilon check` semantics) either as-is or
//! auto-wrapped — a standard corelib prelude plus a `^ = () -> $ => < … >`
//! body, which lets fragments (computed bindings, method chains, bare
//! expressions) check without ceremony. A fence marked ```quilon ignore is
//! skipped: it is deliberately not a compilable program (an error demo,
//! pseudo-syntax, future syntax, or a fragment leaning on names defined only
//! in the surrounding prose).

use quilon::driver::front_end;
use std::path::{Path, PathBuf};

/// Every corelib module, so a fragment may use `print`, `failAt`, `now`, `Request`, …
const PRELUDE: &str =
    "<< core.io\n<< core.test\n<< core.cli\n<< core.time\n<< core.net\n<< core.http\n";

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs")
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            md_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "md") {
            out.push(path);
        }
    }
}

/// Strip one level of blockquote prefix (`> ` or a bare `>`), used only for
/// fences that open inside a blockquote — a plain fence's `>` lines are
/// Quilon block closes and must stay untouched.
fn dequote(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("> ") {
        rest
    } else if trimmed == ">" {
        ""
    } else {
        line
    }
}

struct Fence {
    /// 1-based line of the ```quilon opener in the markdown file.
    line: usize,
    body: String,
    ignored: bool,
}

fn fences_in(path: &Path) -> Vec<Fence> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let lines: Vec<&str> = text.lines().collect();
    let mut fences = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let quoted = lines[i].trim_start().starts_with('>');
        let opener = if quoted { dequote(lines[i]) } else { lines[i] };
        let opener = opener.trim_start();
        if let Some(info) = opener.strip_prefix("```") {
            let info = info.trim();
            let start = i + 1;
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                let raw = if quoted { dequote(lines[i]) } else { lines[i] };
                if raw.trim_start().starts_with("```") {
                    break;
                }
                body.push(raw);
                i += 1;
            }
            if info == "quilon" || info == "quilon ignore" {
                fences.push(Fence {
                    line: start,
                    body: body.join("\n"),
                    ignored: info == "quilon ignore",
                });
            }
        }
        i += 1;
    }
    fences
}

/// Wrap a fragment: the corelib prelude, then the body inside a `^` block.
fn wrapped(body: &str) -> String {
    let indented: Vec<String> = body
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else {
                format!("  {l}")
            }
        })
        .collect();
    format!(
        "{PRELUDE}^ = () -> $ => <\n{}\n  $\n>\n",
        indented.join("\n")
    )
}

/// Front-end a source string via a temp file (front_end takes a path).
fn check_source(dir: &Path, n: usize, source: &str) -> Result<(), String> {
    let path = dir.join(format!("doc_sample_{n}.qn"));
    std::fs::write(&path, source).expect("write temp sample");
    let result = front_end(&path).map(|_| ()).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    result
}

#[test]
fn every_doc_sample_compiles() {
    let mut files = Vec::new();
    md_files(&docs_dir(), &mut files);
    assert!(!files.is_empty(), "no markdown found under docs/");

    let tmp = std::env::temp_dir().join(format!("quilon_doc_samples_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    let mut total = 0;
    let mut failures = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(docs_dir().parent().unwrap())
            .unwrap_or(file);
        for fence in fences_in(file) {
            if fence.ignored {
                continue;
            }
            total += 1;
            if check_source(&tmp, total, &fence.body).is_ok() {
                continue;
            }
            // Not a standalone program — a fragment may still check wrapped.
            if let Err(e) = check_source(&tmp, total, &wrapped(fence.body.as_str())) {
                failures.push(format!("{}:{}\n{e}", rel.display(), fence.line));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        total > 0,
        "no checkable ```quilon fences under docs/ — the gate would pass by iterating nothing"
    );
    assert!(
        failures.is_empty(),
        "{} doc sample(s) failed to compile (as-is and wrapped):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
