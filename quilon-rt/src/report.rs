// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Located fail-loud reports: the runtime half of the call-site machinery.
//!
//! A failing `core.test` assertion reports its call site as a framed diagnostic, composed
//! in Quilon (`corelib/test.ql`'s `failAt`). The runtime's OWN fail-loud checks — an
//! invalid `arr[i]`, a `Text.replace`/`repeat` contract violation — have no Quilon frame to
//! compose from: they abort from inside an intrinsic. This module renders the same shape for
//! them, from the [`QlSite`] the code generator hands in at each check.
//!
//! The two renderers are separate on purpose (the assertion one stays pure Quilon, which is
//! what makes `core.test` hackable), so they are pinned to the same byte-level shape by
//! `tests/fail_loud_location_test.rs`: an assertion failure and a bounds failure at the same
//! call must frame identically.

use crate::io::{__color_enabled, write_to_fd};
use crate::mem::{QlSlice, format_num};
use crate::process::__exit;
use std::os::raw::c_int;

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

/// ANSI styling for a report, or nothing at all when stderr is not a terminal that wants
/// it. Mirrors the four styles `core.test`'s `failAt` uses.
#[derive(Default)]
struct Style {
    position: &'static str,
    problem: &'static str,
    frame: &'static str,
    plain: &'static str,
}

impl Style {
    fn for_stderr() -> Self {
        match __color_enabled(2) {
            0 => Style::default(),
            _ => Style {
                position: "\x1b[36m",
                problem: "\x1b[1;31m",
                frame: "\x1b[2m",
                plain: "\x1b[0m",
            },
        }
    }
}

/// Report `message` at `site` and terminate with `code`.
///
/// The frame is the one a compiler error uses — position, message, the source line, and a
/// caret run under the failing expression:
///
/// ```text
/// demo.ql:5:11:
/// index 7 out of bounds for an array of size 3
///   |
/// 5 |   value = items[7]
///   |           ^^^^^^^^
/// ```
///
/// A site with no source to show — an empty `file` (a program assembled in memory rather
/// than read from one) or an empty `excerpt` (a position past the last line that has text) —
/// prints the message on its own rather than framing it around a position that would be made
/// up, matching what `crate::diagnostic` does with the same case. A null site means the
/// caller predates this plumbing and is treated the same way.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a `QlSite` whose slices point to valid UTF-8 for their length.
pub(crate) fn fail_at(site: *const QlSite, message: &str, code: c_int) -> ! {
    let mut out = String::new();
    match unsafe { site.as_ref() } {
        Some(site) if !site.file.is_empty() && !site.excerpt.is_empty() => {
            let style = Style::for_stderr();
            let (file, excerpt) = (site.file.as_text(), site.excerpt.as_text());
            let line = format_num(site.line);
            let gutter = " ".repeat(line.chars().count());
            let lead = " ".repeat(site.column.max(1.0) as usize - 1);
            let carets = "^".repeat(site.width.max(1.0) as usize);

            out.push_str(&format!(
                "{}{}:{line}:{}:{}\n",
                style.position,
                shorten_path(&file),
                format_num(site.column),
                style.plain
            ));
            out.push_str(&format!("{}{message}{}\n", style.problem, style.plain));
            out.push_str(&format!("{}{gutter} |{}\n", style.frame, style.plain));
            out.push_str(&format!(
                "{}{line} |{} {excerpt}\n",
                style.frame, style.plain
            ));
            out.push_str(&format!(
                "{}{gutter} |{} {lead}{}{carets}{}\n",
                style.frame, style.plain, style.problem, style.plain
            ));
        }
        _ => {
            out.push_str(message);
            out.push('\n');
        }
    }
    write_to_fd(2, out.as_bytes());
    __exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_path_is_left_alone() {
        assert_eq!(shorten_path("examples/arrays.ql"), "examples/arrays.ql");
    }

    #[test]
    fn a_long_path_keeps_its_end_behind_an_ellipsis() {
        let long = format!("/{}/deeply/nested/module.ql", "a".repeat(80));
        let short = shorten_path(&long);
        assert_eq!(short.chars().count(), MAX_PATH_WIDTH);
        assert!(short.starts_with('…'), "{short}");
        assert!(
            short.ends_with("/deeply/nested/module.ql"),
            "the file name and its nearest directories must survive: {short}"
        );
    }

    #[test]
    fn shortening_counts_characters_not_bytes() {
        // Measured in characters, so the result is never cut mid-character and is never
        // shortened more than it has to be.
        let path = format!("/{}/é.ql", "é".repeat(70));
        let short = shorten_path(&path);
        assert_eq!(short.chars().count(), MAX_PATH_WIDTH);
        assert!(short.ends_with("/é.ql"), "{short}");
    }
}
