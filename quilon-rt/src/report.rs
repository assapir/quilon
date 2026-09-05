// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Located fail-loud reports: the runtime half of the call-site machinery.
//!
//! Everything that reports at a call site frames it here — a failing `assert`/`expect`, and
//! the runtime's own fail-loud checks (an invalid `arr[i]`, a `Text.replace`/`repeat`
//! contract violation), all from the [`QlSite`] the code generator hands in at the check.
//!
//! The frame is the compiler's own (`diagnostic::Diagnostic::render`, drawn by `miette`),
//! composed here by hand since a compiled program carries no renderer;
//! `tests/fail_loud_location_test.rs` pins the two to the same byte-level shape, so a
//! compile error and a runtime failure read identically.

use crate::io::{__color_enabled, write_to_fd};
use crate::mem::{QlSlice, format_num};
use crate::process::__exit;
use crate::test_registry::{Failure, mark_case_failed};
use std::os::raw::c_int;

/// The exit status a failing `assert` leaves — the Rust-panic convention, so a self-verifying
/// program fails loudly in CI.
pub const ASSERTION_EXIT_CODE: c_int = 101;

/// The exit status every OTHER fail-loud runtime check leaves — an invalid `arr[i]`, a
/// failed allocation, a range endpoint that is not a whole number, a match no arm matched,
/// an `@` primitive that could not do what it was asked.
pub(crate) const RUNTIME_EXIT_CODE: c_int = 1;

/// A call site as the code generator materializes it — the runtime mirror of the built-in
/// `Site` record (`file`, `line`, `column`, `excerpt`, `width`), in declaration order.
///
/// `#[repr(C)]` so the field offsets match the LLVM struct codegen emits for the record
/// (`{ {ptr,i64}, double, double, {ptr,i64}, double }`). An intrinsic that can fail takes a
/// pointer to one of these constants; nothing writes through it.
#[repr(C)]
pub struct QlSite {
    pub file: QlSlice,
    pub line: f64,
    pub column: f64,
    pub excerpt: QlSlice,
    pub width: f64,
}

/// How wide a path may be in a report's position line before it is shortened. Wide enough
/// for a realistic project-relative path, narrow enough that the line still fits a terminal
/// beside the `line:column` that follows it.
pub const MAX_PATH_WIDTH: usize = 60;

/// A path shortened to fit a report, keeping its END — the file name and its nearest
/// directories, which is the part a reader needs — behind a leading `…`.
///
/// A location prints the path as the compiler resolved it, which for an absolute path (or a
/// temp directory, or a deeply nested module) can be longer than the terminal is wide, and a
/// wrapped position line is much harder to scan than an elided one. Lives here rather than in
/// the compiler because the runtime renders reports too and cannot depend on it.
pub fn shorten_path(path: &str) -> String {
    let width = path.chars().count();
    match width > MAX_PATH_WIDTH {
        false => path.to_string(),
        true => {
            let kept: String = path.chars().skip(width - (MAX_PATH_WIDTH - 1)).collect();
            format!("…{kept}")
        }
    }
}

/// The runtime's error codes — the numbers the compiler's registry (`diagnostic::codes`)
/// assigns to the failures a compiled program can report. A program reports without the
/// compiler, so the numbers live here too; the compiler's tests pin the two.
pub mod codes {
    pub const ASSERTION_FAILED: u16 = 500;
    pub const INDEX_OUT_OF_BOUNDS: u16 = 501;
    pub const RANGE_ENDPOINT_NOT_WHOLE: u16 = 502;
    pub const MATCH_FAILED: u16 = 503;
    pub const ALLOCATION_FAILED: u16 = 504;
    pub const READ_FAILED: u16 = 505;
    pub const REPLACE_ALL_EMPTY_FROM: u16 = 506;
}

/// ANSI styling for a report, or nothing at all when stderr is not a terminal that wants
/// it. The three styles are the ones the compiler's renderer paints the same frame with.
#[derive(Default)]
struct Style {
    problem: &'static str,
    frame: &'static str,
    plain: &'static str,
}

impl Style {
    fn for_stderr() -> Self {
        match __color_enabled(2) {
            0 => Style::default(),
            _ => Style {
                problem: "\x1b[31m",
                frame: "\x1b[2m",
                plain: "\x1b[0m",
            },
        }
    }
}

/// Report `message` under error code `code` at `site` and terminate with `exit_code`.
///
/// The frame is the one a compiler error uses — the code and message, the position, the
/// source line, and an underline beneath the failing expression:
///
/// ```text
/// error[QN501]: index 7 out of bounds for an array of size 3
///    ╭─[demo.qn:5:11]
///  5 │   value = items[7]
///    ·           ────────
///    ╰────
/// ```
///
/// A site with no source to show — an empty `file` (a program assembled in memory rather
/// than read from one) or an empty `excerpt` (a position past the last line that has text) —
/// prints the header on its own rather than framing it around a position that would be made
/// up, matching what the compiler does with the same case. A null site means the caller
/// predates this plumbing and is treated the same way.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a `QlSite` whose slices point to valid UTF-8 for their length.
pub(crate) fn fail_at(site: *const QlSite, code: u16, message: &str, exit_code: c_int) -> ! {
    report_at(site, code, message);
    __exit(exit_code)
}

/// Report `message` under `code` at `site` on stderr and RETURN — the recorded half of the
/// same frame [`fail_at`] ends the process with.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a `QlSite` whose slices point to valid UTF-8 for their length.
pub(crate) fn report_at(site: *const QlSite, code: u16, message: &str) {
    let style = Style::for_stderr();
    let mut out = format!(
        "{}error[QN{code:03}]:{} {message}\n",
        style.problem, style.plain
    );
    if let Some(site) = unsafe { site.as_ref() }
        && !site.file.is_empty()
        && !site.excerpt.is_empty()
    {
        let (file, excerpt) = (site.file.as_text(), site.excerpt.as_text());
        let line = format_num(site.line);
        let gutter = " ".repeat(line.chars().count() + 2);
        let lead = " ".repeat(site.column.max(1.0) as usize - 1);
        let underline = "─".repeat(site.width.max(1.0) as usize);

        out.push_str(&format!(
            "{}{gutter}╭─[{}:{line}:{}]{}\n",
            style.frame,
            shorten_path(&file),
            format_num(site.column),
            style.plain
        ));
        out.push_str(&format!(
            "{} {line} │{} {excerpt}\n",
            style.frame, style.plain
        ));
        out.push_str(&format!(
            "{}{gutter}·{} {lead}{}{underline}{}\n",
            style.frame, style.plain, style.problem, style.plain
        ));
        out.push_str(&format!("{}{gutter}╰────{}\n", style.frame, style.plain));
    }
    write_to_fd(2, out.as_bytes());
}

/// A failing `assert(actual, matcher)`: report `message` at the assertion's own call site and
/// terminate with [`ASSERTION_EXIT_CODE`]. Never returns.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid `QlSite`; `message`/`length` are a UTF-8 `Text`.
#[unsafe(no_mangle)]
pub extern "C" fn __assert_failed(site: *const QlSite, message: *const u8, length: i64) -> ! {
    fail_at(
        site,
        codes::ASSERTION_FAILED,
        &message_text(message, length),
        ASSERTION_EXIT_CODE,
    )
}

/// A failing `expect(actual, matcher)`: report `message` at the assertion's own call site,
/// mark the running case failed, and RETURN. The case's remaining assertions see the mark and
/// do nothing; the suite carries on with the next case.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid `QlSite`; `message`/`length` are a UTF-8 `Text`.
#[unsafe(no_mangle)]
pub extern "C" fn __expect_failed(site: *const QlSite, message: *const u8, length: i64) {
    let message = message_text(message, length);
    report_at(site, codes::ASSERTION_FAILED, &message);
    let (file, line) = location_of(site);
    mark_case_failed(Failure {
        message,
        file,
        line,
    });
}

/// The file and line `site` names, as a JSON reporter carries them — empty and 0 for a
/// site with no source (see [`report_at`]).
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a `QlSite` whose slices point to valid UTF-8 for their length.
fn location_of(site: *const QlSite) -> (String, u64) {
    match unsafe { site.as_ref() } {
        Some(site) => (site.file.as_text().into_owned(), site.line as u64),
        None => (String::new(), 0),
    }
}

/// A `?`/`|` match no arm matched: report at `site` (the match expression's own location)
/// and terminate with [`RUNTIME_EXIT_CODE`]. Never returns.
///
/// The checker requires every match to be total, so this is the backstop for what it cannot
/// prove — reached only if that guarantee is broken, which is why it fails loudly instead of
/// letting the match yield a result slot no arm wrote.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
#[unsafe(no_mangle)]
pub extern "C" fn __match_fail(site: *const QlSite) -> ! {
    fail_at(
        site,
        codes::MATCH_FAILED,
        "no arm of this match matched the value",
        RUNTIME_EXIT_CODE,
    )
}

/// A `Text` argument's bytes as a `String`. Length is clamped at 0, and invalid UTF-8 is
/// replaced rather than aborting: a diagnostic is the wrong place to fail.
fn message_text(message: *const u8, length: i64) -> String {
    if message.is_null() || length <= 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(message, length as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_path_is_left_alone() {
        assert_eq!(shorten_path("examples/arrays.qn"), "examples/arrays.qn");
    }

    #[test]
    fn a_long_path_keeps_its_end_behind_an_ellipsis() {
        let long = format!("/{}/deeply/nested/module.qn", "a".repeat(80));
        let short = shorten_path(&long);
        assert_eq!(short.chars().count(), MAX_PATH_WIDTH);
        assert!(short.starts_with('…'), "{short}");
        assert!(
            short.ends_with("/deeply/nested/module.qn"),
            "the file name and its nearest directories must survive: {short}"
        );
    }

    #[test]
    fn shortening_counts_characters_not_bytes() {
        // Measured in characters, so the result is never cut mid-character and is never
        // shortened more than it has to be.
        let path = format!("/{}/é.qn", "é".repeat(70));
        let short = shorten_path(&path);
        assert_eq!(short.chars().count(), MAX_PATH_WIDTH);
        assert!(short.ends_with("/é.qn"), "{short}");
    }
}
