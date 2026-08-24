//! Runtime benchmark: how fast the programs Quilon *emits* actually run, and how long
//! the compiler takes end to end from the command line.
//!
//! The compile-speed benchmark next door measures the compiler. Nothing measured the
//! output: checked indexing added a bounds test per access, ranges materialize whole
//! arrays, `.length` walks the text, and everything is emitted at `OptimizationLevel::
//! None` — all of it unmeasured, so a regression in generated-code speed was invisible.
//!
//! Run it with `cargo bench --bench runtime_speed`. Like the compile family, the corpora
//! are committed under `benches/runtime/` and `--regen` rewrites them; what runs is the
//! committed bytes. Nothing here asserts — it prints tables for trend tracking.
//!
//! Each program is built once and then run `RUNS` times; the reported figure is the best
//! run, not the mean, because the fastest observation is the one least polluted by
//! whatever else the machine was doing. Peak RSS comes from the kernel's own accounting
//! of the child (`wait4`), so it is the real high-water mark rather than a sample.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How many times each program is run. Best-of, so a handful is enough.
const RUNS: u32 = 5;

/// Where the runtime corpora live, relative to the crate root.
const RUNTIME_DIR: &str = "benches/runtime";

/// The programs, in table order: file stem, and what each one exercises.
#[path = "series.rs"]
mod series;
use series::Trend;

const PROGRAMS: &[(&str, &str)] = &[
    ("tco_loop", "50M-iteration tail-recursive countdown"),
    ("array_pipeline", "map/filter/reduce over a 2M range"),
    ("text_loop", "400k interpolated strings"),
    ("gc_churn", "3M short-lived arrays"),
];

fn main() {
    if std::env::args().any(|a| a == "--regen") {
        regenerate();
        return;
    }

    let quilon = Path::new(env!("CARGO_BIN_EXE_quilon"));
    let workdir = std::env::temp_dir().join("quilon-runtime-bench");
    let _ = std::fs::create_dir_all(&workdir);

    // `--baseline <path>` compares against a previous run, `--metrics <path>` records this
    // one. Both absent prints the tables exactly as they always have.
    let mut trend = Trend::from_args("runtime_speed");
    let (header, rule) = match trend.has_baseline() {
        true => (
            "| program | shape | build | run | Δ run | peak RSS | Δ RSS |",
            "|---|---|--:|--:|--:|--:|--:|",
        ),
        false => (
            "| program | shape | build | run | peak RSS |",
            "|---|---|--:|--:|--:|",
        ),
    };

    println!("Runtime benchmark — best of {RUNS} runs\n");
    println!("{header}");
    println!("{rule}");
    for (stem, shape) in PROGRAMS {
        let source = runtime_dir().join(format!("{stem}.ql"));
        let binary = workdir.join(stem);

        let build = measure(Command::new(quilon).args([
            "build".as_ref(),
            source.as_os_str(),
            "-o".as_ref(),
            binary.as_os_str(),
        ]));
        assert!(build.ok, "building {stem} failed");

        let mut best = Measured::worst();
        for _ in 0..RUNS {
            let run = measure(&mut Command::new(&binary));
            assert!(run.ok, "running {stem} failed");
            best = best.min(run);
        }
        // `build` is recorded but has no printed delta: it is the compiler's cost, which the
        // compile-speed family measures properly. What this family is about is `run` and the
        // memory the emitted program uses.
        trend.delta(stem, "build", build.wall.as_secs_f64() * 1000.0);
        let run_delta = trend.delta(stem, "run", best.wall.as_secs_f64() * 1000.0);
        let rss_delta = trend.delta(
            stem,
            "peak RSS",
            best.peak_rss_kb.unwrap_or(0) as f64 / 1024.0,
        );
        let row = format!(
            "| `{stem}` | {shape} | {} | {} |",
            ms(build.wall),
            ms(best.wall),
        );
        match trend.has_baseline() {
            true => println!(
                "{row} {run_delta} | {} | {rss_delta} |",
                rss(best.peak_rss_kb)
            ),
            false => println!("{row} {} |", rss(best.peak_rss_kb)),
        }
    }
    println!();

    latency_table(quilon, &workdir, &mut trend);
    trend.finish();
}

/// What a user waits for: the whole command, including process start, JIT set-up or the
/// link, and — the first time on a machine — extracting the embedded runtime archive.
///
/// Two things have to be undone for this to measure a shipped binary rather than a
/// build tree. The compiler is copied somewhere on its own, because `quilon build` takes
/// `libquilon_rt.a` from beside the running binary when it is there. And
/// `QUILON_RT_LIB` is cleared from the child's environment: the build script bakes that
/// override and cargo hands it to anything it runs, so without clearing it the compiler
/// links straight against the build tree's archive and never consults a cache at all —
/// which is exactly what made the first version of this table report two warm rows.
fn latency_table(quilon: &Path, workdir: &Path, trend: &mut Trend) {
    let tiny = workdir.join("tiny.ql");
    std::fs::write(&tiny, "^ = () -> Num => 0\n").expect("writing the latency program");
    let out = workdir.join("tiny");

    let standalone = workdir.join("quilon-standalone");
    std::fs::copy(quilon, &standalone).expect("copying the compiler somewhere on its own");
    let quilon = &standalone;

    // The same one-liner with a library import. Almost every real program has one, and it
    // is a different cost: importing pulls the module's whole source through the front end
    // and (before emission-side pruning) emitted every function it defined. The import-free
    // row above cannot see any of that, so it stays as the floor and this one sits beside it.
    let tiny_import = workdir.join("tiny_import.ql");
    std::fs::write(
        &tiny_import,
        "<< core.test\n^ = () -> $ => assertEq(1 + 1, 2)\n",
    )
    .expect("writing the importing latency program");

    println!("Command latency — best of {RUNS} runs, on a one-line program\n");
    match trend.has_baseline() {
        true => {
            println!("| command | cache | wall | Δ wall | peak RSS | Δ RSS |");
            println!("|---|---|--:|--:|--:|--:|");
        }
        false => {
            println!("| command | cache | wall | peak RSS |");
            println!("|---|---|--:|--:|");
        }
    }

    for (label, program) in [
        ("`quilon run`", &tiny),
        ("`quilon run`, `<< core.test`", &tiny_import),
    ] {
        let run = best_of(|| {
            measure(
                Command::new(quilon)
                    .env_remove("QUILON_RT_LIB")
                    .args(["run".as_ref(), program.as_os_str()]),
            )
        });
        let wall_delta = trend.delta(label, "wall", run.wall.as_secs_f64() * 1000.0);
        let rss_delta = trend.delta(
            label,
            "peak RSS",
            run.peak_rss_kb.unwrap_or(0) as f64 / 1024.0,
        );
        let row = format!("| {label} | n/a | {} |", ms(run.wall));
        match trend.has_baseline() {
            true => println!(
                "{row} {wall_delta} | {} | {rss_delta} |",
                rss(run.peak_rss_kb)
            ),
            false => println!("{row} {} |", rss(run.peak_rss_kb)),
        }
    }

    // A cold cache means the embedded `libquilon_rt.a` has to be extracted before the
    // link; pointing the cache at a fresh directory reproduces a first run on a new
    // machine without touching the real one.
    for (label, cache) in [("cold", Some(())), ("warm", None)] {
        let dir = workdir.join(format!("cache-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        if cache.is_none() {
            // Warm: run once first so the archive is already extracted.
            let _ = measure(
                Command::new(quilon)
                    .env_remove("QUILON_RT_LIB")
                    .env("XDG_CACHE_HOME", &dir)
                    .args([
                        "build".as_ref(),
                        tiny.as_os_str(),
                        "-o".as_ref(),
                        out.as_os_str(),
                    ]),
            );
        }
        let build = best_of(|| {
            if cache.is_some() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            let m = measure(
                Command::new(quilon)
                    .env_remove("QUILON_RT_LIB")
                    .env("XDG_CACHE_HOME", &dir)
                    .args([
                        "build".as_ref(),
                        tiny.as_os_str(),
                        "-o".as_ref(),
                        out.as_os_str(),
                    ]),
            );
            // The cold number is only meaningful if the runtime archive really had to be
            // extracted into this cache. If it did not, the compiler found the archive
            // somewhere else and the row would silently be a second warm measurement.
            assert!(
                extracted_into(&dir),
                "cache {dir:?} is empty after a build — the runtime archive came from \
                 somewhere else, so this row would not be measuring a cold cache"
            );
            m
        });
        // Labelled by cache state, since `quilon build` appears twice with very different
        // numbers and a series row has to tell them apart.
        let row_label = format!("quilon build ({label} cache)");
        let wall_delta = trend.delta(&row_label, "wall", build.wall.as_secs_f64() * 1000.0);
        let rss_delta = trend.delta(
            &row_label,
            "peak RSS",
            build.peak_rss_kb.unwrap_or(0) as f64 / 1024.0,
        );
        let row = format!("| `quilon build` | {label} | {} |", ms(build.wall));
        match trend.has_baseline() {
            true => println!(
                "{row} {wall_delta} | {} | {rss_delta} |",
                rss(build.peak_rss_kb)
            ),
            false => println!("{row} {} |", rss(build.peak_rss_kb)),
        }
    }
    println!();
}

/// Whether a runtime archive was extracted into `cache` (the `quilon/…​.a` it writes).
fn extracted_into(cache: &Path) -> bool {
    std::fs::read_dir(cache.join("quilon"))
        .map(|mut entries| entries.any(|e| e.is_ok()))
        .unwrap_or(false)
}

fn best_of(mut once: impl FnMut() -> Measured) -> Measured {
    let mut best = Measured::worst();
    for _ in 0..RUNS {
        let m = once();
        assert!(m.ok, "benchmarked command failed");
        best = best.min(m);
    }
    best
}

/// One observation of a child process: how long it took, its peak resident set, and
/// whether it succeeded.
#[derive(Clone, Copy)]
struct Measured {
    wall: Duration,
    peak_rss_kb: Option<i64>,
    ok: bool,
}

impl Measured {
    fn worst() -> Self {
        Self {
            wall: Duration::MAX,
            peak_rss_kb: None,
            ok: true,
        }
    }

    /// Keep the faster observation, carrying its memory figure with it.
    fn min(self, other: Self) -> Self {
        if other.wall < self.wall { other } else { self }
    }
}

/// Run `cmd` to completion, timing it and asking the kernel for its peak resident set.
///
/// `wait4` reports what the child actually used, rather than a sampled guess, so the
/// number is exact; the child is reaped there, which is why the handle is forgotten
/// instead of waited on a second time.
fn measure(cmd: &mut Command) -> Measured {
    let start = Instant::now();
    let child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawning the benchmarked command");
    let pid = child.id() as i32;

    let mut status: libc::c_int = 0;
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let waited = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
    let wall = start.elapsed();
    std::mem::forget(child);

    let exited_ok = waited == pid && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
    Measured {
        wall,
        // `ru_maxrss` is kilobytes on Linux.
        peak_rss_kb: (waited == pid).then_some(usage.ru_maxrss),
        ok: exited_ok,
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

fn rss(kb: Option<i64>) -> String {
    match kb {
        Some(kb) => format!("{:.1} MB", kb as f64 / 1024.0),
        None => "—".to_string(),
    }
}

fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_DIR)
}

/// Rewrite `benches/runtime/` from the generators below — `cargo bench --bench
/// runtime_speed -- --regen`. Same reasoning as the compile family: changing a corpus
/// should be a reviewable diff, not something that happens on the next run.
fn regenerate() {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("creating {dir:?}: {e}"));
    for (stem, source) in [
        ("tco_loop", tco_loop(50_000_000)),
        ("array_pipeline", array_pipeline(2_000_000)),
        ("text_loop", text_loop(400_000)),
        ("gc_churn", gc_churn(3_000_000)),
    ] {
        let path = dir.join(format!("{stem}.ql"));
        std::fs::write(&path, source).unwrap_or_else(|e| panic!("writing {path:?}: {e}"));
        println!("wrote {}", path.display());
    }
}

/// A self-tail-call lowered to a loop: measures the loop itself, with no allocation.
fn tco_loop(iterations: u64) -> String {
    let mut src = String::new();
    let _ = writeln!(
        src,
        "~ A tail-recursive countdown — lowered to a loop, so this times raw iteration."
    );
    let _ = writeln!(
        src,
        "count = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : count(n - 1, acc + 1)"
    );
    let _ = writeln!(src, "^ = () -> Num => count({iterations}, 0) > 0 ? 0 : 1");
    src
}

/// The array-method pipeline: a materialized range, then three passes over it, each
/// allocating its own result.
fn array_pipeline(size: u64) -> String {
    let mut src = String::new();
    let _ = writeln!(
        src,
        "~ Range materialization plus map/filter/reduce — three passes, each allocating."
    );
    let _ = writeln!(src, "^ = () -> Num => <");
    let _ = writeln!(src, "  xs = 1 <- {size}");
    let _ = writeln!(
        src,
        "  total = xs.map(x => x * 2).filter(x => x > 100).reduce(0, (a, x) => a + x)"
    );
    let _ = writeln!(src, "  total > 0 ? 0 : 1");
    let _ = writeln!(src, ">");
    src
}

/// Text built through interpolation in a loop: renders values and concatenates, so it
/// measures the render path and the allocation behind every intermediate `Text`.
fn text_loop(iterations: u64) -> String {
    let mut src = String::new();
    let _ = writeln!(
        src,
        "~ Interpolation in a loop: each step renders two values and measures the result."
    );
    let _ = writeln!(
        src,
        "build = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : build(n - 1, acc + \"item `n` of `n * 2`\".size)"
    );
    let _ = writeln!(src, "^ = () -> Num => build({iterations}, 0) > 0 ? 0 : 1");
    src
}

/// Short-lived allocations: every iteration builds an array that dies immediately, so
/// the GC does all the work.
fn gc_churn(iterations: u64) -> String {
    let mut src = String::new();
    let _ = writeln!(
        src,
        "~ Allocate an array per iteration and drop it — the collector's problem, not the loop's."
    );
    let _ = writeln!(
        src,
        "churn = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : churn(n - 1, acc + [n, n + 1, n + 2].size)"
    );
    let _ = writeln!(src, "^ = () -> Num => churn({iterations}, 0) > 0 ? 0 : 1");
    src
}
