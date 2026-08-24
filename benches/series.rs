// A benchmark's numbers, written for the next run to compare against.
//
// Both families (`compile_speed`, `runtime_speed`) include this file. CI keeps one series
// across runs on `main` and hands the previous run's copy back as a baseline, so each table
// carries a delta column and a regression shows up in the run that introduced it. Nothing
// here gates anything: shared-runner numbers are noisy in absolute terms, and only
// interleaved runs on one machine compare credibly. A delta is a hint about where to look.
//
// The format is one row per measurement, tab-separated:
//
//     compile_speed<TAB>flat<TAB>codegen<TAB>37.4
//
// Values are plain `f64` in the unit the table prints (milliseconds, megabytes), so a series
// file is greppable and diffable by hand. An unknown row is not an error in either direction:
// a corpus added since the baseline simply has no delta, and one removed is ignored.

// Included by both bench targets AND by `tests/bench_series_test.rs`, each of which uses a
// different subset of this file — so an item unused by one of them is not dead code.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt::Write as _;

/// One family's measurements, keyed by `(row label, metric)`.
#[derive(Default)]
pub struct Series {
    values: HashMap<(String, String), f64>,
}

impl Series {
    /// Read a series file, keeping only the rows belonging to `family`. A missing or
    /// unreadable file is an EMPTY series, not an error — a first run, an evicted cache, and
    /// a fork with no baseline all take that path, and each prints its table as it always did.
    pub fn read(path: &str, family: &str) -> Series {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Series::default();
        };
        let mut values = HashMap::new();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(row_family), Some(row), Some(metric), Some(value)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if row_family == family
                && let Ok(value) = value.parse::<f64>()
            {
                values.insert((row.to_string(), metric.to_string()), value);
            }
        }
        Series { values }
    }

    /// Record one measurement.
    pub fn record(&mut self, row: &str, metric: &str, value: f64) {
        self.values
            .insert((row.to_string(), metric.to_string()), value);
    }

    /// This measurement's value in the series, if it has one.
    pub fn get(&self, row: &str, metric: &str) -> Option<f64> {
        self.values
            .get(&(row.to_string(), metric.to_string()))
            .copied()
    }

    /// Append this family's rows to the series file at `path`, creating it if needed.
    ///
    /// Appending (rather than rewriting) is what lets the two families share one file: each
    /// writes its own rows, and `read` filters by family. Sorted, so a diff between two
    /// series files reads as changed numbers rather than reordered lines.
    pub fn append_to(&self, path: &str, family: &str) -> std::io::Result<()> {
        let mut rows: Vec<(&(String, String), &f64)> = self.values.iter().collect();
        rows.sort_by_key(|(key, _)| *key);
        let mut out = String::new();
        for ((row, metric), value) in rows {
            let _ = writeln!(out, "{family}\t{row}\t{metric}\t{value}");
        }
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(out.as_bytes())
    }
}

/// How a family was asked to compare and record: the paths, if any, from the command line.
///
/// `--baseline <path>` is the previous run's series (compared against, never written);
/// `--metrics <path>` is where this run's numbers go. Either may be absent, and absent is
/// the ordinary local case — the tables print exactly as they did before.
pub struct Trend {
    baseline: Series,
    metrics_path: Option<String>,
    family: &'static str,
    current: Series,
}

impl Trend {
    /// A trend over an explicit baseline. `from_args` is the command-line convenience over
    /// this; `tests/bench_series_test.rs` builds one directly.
    pub fn new(baseline: Series, metrics_path: Option<String>, family: &'static str) -> Trend {
        Trend {
            baseline,
            metrics_path,
            family,
            current: Series::default(),
        }
    }

    /// Read `--baseline` / `--metrics` out of the command line for `family`.
    pub fn from_args(family: &'static str) -> Trend {
        let args: Vec<String> = std::env::args().collect();
        let value_after = |flag: &str| -> Option<String> {
            args.iter()
                .position(|a| a == flag)
                .and_then(|at| args.get(at + 1))
                .cloned()
        };
        let baseline = match value_after("--baseline") {
            Some(path) => Series::read(&path, family),
            None => Series::default(),
        };
        Trend::new(baseline, value_after("--metrics"), family)
    }

    /// Record a measurement and render the delta against the baseline for the table.
    ///
    /// Returns the cell to print: empty when there is nothing to compare against (no
    /// baseline, or a row the baseline does not have), so a table without a baseline looks
    /// exactly as it always has.
    pub fn delta(&mut self, row: &str, metric: &str, value: f64) -> String {
        self.current.record(row, metric, value);
        let Some(before) = self.baseline.get(row, metric) else {
            return String::new();
        };
        // A percentage, not an absolute: the rows span three orders of magnitude, and "+8%"
        // is the readable form for all of them. Under a tenth of a percent is called even —
        // it is the same number with a rounding difference.
        let change = match before == 0.0 {
            true => return String::new(),
            false => (value - before) / before * 100.0,
        };
        match change.abs() < 0.1 {
            true => "±0%".to_string(),
            false => format!("{change:+.1}%"),
        }
    }

    /// Write this run's numbers where `--metrics` asked for them. Called once, after the
    /// tables are printed; a write failure is reported and ignored, since a benchmark that
    /// measured fine should not fail over its bookkeeping.
    pub fn finish(&self) {
        let Some(path) = &self.metrics_path else {
            return;
        };
        if let Err(error) = self.current.append_to(path, self.family) {
            eprintln!("could not write {path}: {error}");
        }
    }

    /// Whether a baseline was supplied and had anything in it — what decides whether the
    /// tables carry a delta column at all.
    pub fn has_baseline(&self) -> bool {
        !self.baseline.values.is_empty()
    }
}
