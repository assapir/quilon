//! The benchmark series format: what a run records, and how the next run's delta column is
//! computed from it.
//!
//! The logic lives in `benches/series.rs`, which both bench targets include. Those are
//! `harness = false` targets, so a `#[cfg(test)]` module inside them would never run under
//! `cargo test` — these tests live here instead, including the same source file, so the
//! format is gated by the ordinary suite.

#[path = "../benches/series.rs"]
mod series;
use series::{Series, Trend};

/// A unique scratch path, since tests in a binary run in parallel.
fn scratch(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("quilon-series-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir.join(format!("{tag}.tsv"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_series_round_trips_through_a_file() {
    let path = scratch("round_trip");
    let _ = std::fs::remove_file(&path);
    let mut written = Series::default();
    written.record("flat", "codegen", 37.4);
    written.record("flat", "total", 50.1);
    written.append_to(&path, "compile_speed").expect("write");

    let read = Series::read(&path, "compile_speed");
    assert_eq!(read.get("flat", "codegen"), Some(37.4));
    assert_eq!(read.get("flat", "total"), Some(50.1));
    assert_eq!(read.get("flat", "lex"), None);
}

/// Both families append to one file, so each has to read back only its own rows — a corpus
/// and a program can share a name without one shadowing the other.
#[test]
fn two_families_share_one_file_without_seeing_each_other() {
    let path = scratch("two_families");
    let _ = std::fs::remove_file(&path);
    let mut compile = Series::default();
    compile.record("shared_name", "total", 50.1);
    compile.append_to(&path, "compile_speed").expect("write");
    let mut runtime = Series::default();
    runtime.record("shared_name", "total", 999.0);
    runtime.append_to(&path, "runtime_speed").expect("write");

    assert_eq!(
        Series::read(&path, "compile_speed").get("shared_name", "total"),
        Some(50.1)
    );
    assert_eq!(
        Series::read(&path, "runtime_speed").get("shared_name", "total"),
        Some(999.0)
    );
}

/// No baseline is the ordinary case — a first run, an evicted cache, a fork — and must not
/// be an error: the benchmark still measures, it just has nothing to compare against.
#[test]
fn a_missing_baseline_is_empty_not_an_error() {
    let series = Series::read("/nonexistent/bench-series.tsv", "compile_speed");
    assert_eq!(series.get("flat", "total"), None);
}

#[test]
fn a_malformed_line_is_skipped_rather_than_fatal() {
    let path = scratch("malformed");
    std::fs::write(
        &path,
        "compile_speed\tflat\ttotal\t50.1\ngarbage\ncompile_speed\tflat\tlex\tnot-a-number\n",
    )
    .expect("write");
    let series = Series::read(&path, "compile_speed");
    assert_eq!(series.get("flat", "total"), Some(50.1));
    assert_eq!(series.get("flat", "lex"), None);
}

/// A corpus added since the baseline was recorded has nothing to compare against, and shows
/// no delta rather than a made-up one.
#[test]
fn a_row_the_baseline_does_not_have_gets_no_delta() {
    let mut trend = Trend::new(Series::default(), None, "compile_speed");
    assert_eq!(trend.delta("brand_new_corpus", "total", 12.0), "");
    assert!(!trend.has_baseline());
}

#[test]
fn a_delta_is_a_signed_percentage_of_the_baseline() {
    let mut baseline = Series::default();
    baseline.record("flat", "total", 100.0);
    baseline.record("deep", "total", 100.0);
    baseline.record("records", "total", 100.0);
    baseline.record("zero", "total", 0.0);
    let mut trend = Trend::new(baseline, None, "compile_speed");

    assert!(trend.has_baseline());
    assert_eq!(trend.delta("flat", "total", 108.0), "+8.0%");
    assert_eq!(trend.delta("deep", "total", 92.5), "-7.5%");
    // Under a tenth of a percent is the same number with a rounding difference.
    assert_eq!(trend.delta("records", "total", 100.04), "±0%");
    // A zero baseline has no percentage to give.
    assert_eq!(trend.delta("zero", "total", 5.0), "");
}

/// What a run records is what the next run reads: measure, write, read back, and the deltas
/// come out as zero rather than as anything at all.
#[test]
fn a_recorded_run_is_the_next_runs_baseline() {
    let path = scratch("round_two");
    let _ = std::fs::remove_file(&path);
    let mut first = Trend::new(Series::default(), Some(path.clone()), "compile_speed");
    first.delta("flat", "total", 50.0);
    first.finish();

    let mut second = Trend::new(Series::read(&path, "compile_speed"), None, "compile_speed");
    assert_eq!(second.delta("flat", "total", 50.0), "±0%");
    assert_eq!(second.delta("flat", "total", 55.0), "+10.0%");
}
