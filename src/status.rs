//! What a command says about its own progress, on stderr.
//!
//! Per-stage progress (lexing, parsing, …) exists ONLY on a terminal, and only as a live
//! spinner line that clears itself — nothing from it survives into scrollback. Off a
//! terminal, or with `CI` set in the environment (a pty a CI runner allocates is still not
//! interactive), no stage line prints at all: just the final one-liner (`✓ file (9ms) —
//! quip`) and diagnostics. `--quiet` silences the final line too — diagnostics still print,
//! they are not status. `quilon run` clears the spinner and says nothing more, so the
//! program's own output stands alone. `quilon test` never shows per-stage progress for a
//! suite's compile, even on a terminal — [`Status::compiling`] is the one transient line it
//! gets, cleared before the suite's own case tree prints.

use std::cell::OnceCell;
use std::io::IsTerminal;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};

use crate::diagnostic::Diagnostic;
use crate::quips;
use crate::source_map::SourceMap;

/// A stage of a command, in pipeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Lexing,
    Parsing,
    /// `<<` imports.
    Resolving,
    Checking,
    /// LLVM code generation.
    Generating,
    /// The native link.
    Linking,
}

impl Stage {
    /// The stage's line, in the compiler's voice.
    pub fn line(self) -> &'static str {
        quips::pick(match self {
            Stage::Lexing => quips::LEXING,
            Stage::Parsing => quips::PARSING,
            Stage::Resolving => quips::RESOLVING,
            Stage::Checking => quips::CHECKING,
            Stage::Generating => quips::GENERATING,
            Stage::Linking => quips::LINKING,
        })
    }
}

enum Mode {
    Quiet,
    /// The final line only — no per-stage lines. Stderr is not a terminal, or `CI` is set.
    Plain,
    /// A live spinner that collapses to the final line. Started by the first stage (or the
    /// first [`Status::compiling`]), so a status that only ever reports a failure draws
    /// nothing.
    Live(OnceCell<ProgressBar>),
}

pub struct Status {
    mode: Mode,
    color: bool,
    started: Instant,
}

impl Status {
    /// Says nothing at all.
    pub fn silent() -> Self {
        Self {
            mode: Mode::Quiet,
            color: false,
            started: Instant::now(),
        }
    }

    /// The status a command reports on stderr: silent under `quiet`, a live per-stage
    /// spinner on an interactive terminal, the final line alone otherwise.
    pub fn for_command(quiet: bool) -> Self {
        Self::new(quiet, Mode::Plain)
    }

    /// The status of a command whose output is the program's own (`run`): live on a
    /// terminal, where the spinner leaves no trace, and silent everywhere else.
    pub fn transient(quiet: bool) -> Self {
        Self::new(quiet, Mode::Quiet)
    }

    fn new(quiet: bool, without_terminal: Mode) -> Self {
        let mode = match (quiet, is_interactive()) {
            (true, _) => Mode::Quiet,
            (false, false) => without_terminal,
            (false, true) => Mode::Live(OnceCell::new()),
        };
        Self {
            mode,
            color: color_enabled(),
            started: Instant::now(),
        }
    }

    /// The spinner this status draws through, building it on first use. `Live` only —
    /// callers check the mode first.
    fn spinner(&self) -> Option<&ProgressBar> {
        match &self.mode {
            Mode::Live(spinner) => Some(spinner.get_or_init(|| {
                let spinner = ProgressBar::new_spinner().with_style(
                    ProgressStyle::with_template("{spinner:.cyan} {msg}")
                        .expect("a fixed template"),
                );
                spinner.enable_steady_tick(Duration::from_millis(80));
                spinner
            })),
            _ => None,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Announce that `stage` begins — a live spinner update on an interactive terminal,
    /// nothing anywhere else (see the module docs: there is no non-terminal stage line).
    pub fn stage(&self, stage: Stage) {
        if let Some(spinner) = self.spinner() {
            spinner.set_message(stage.line());
        }
    }

    /// The one progress line `quilon test` shows for a suite's compile: a live "compiling
    /// `what`" spinner update on an interactive terminal, nothing elsewhere. Never the
    /// per-stage lines `stage` draws — a suite's own case tree is the progress that matters.
    pub fn compiling(&self, what: &str) {
        if let Some(spinner) = self.spinner() {
            spinner.set_message(format!("compiling {what}"));
        }
    }

    /// End the command: the final line — `what`, the elapsed time, and `quip` — replacing
    /// the spinner. Nothing under `quiet`.
    pub fn done(&self, what: &str, quip: &str) {
        self.done_with(&format!(
            "{} {what} ({}) — {quip}",
            self.paint("32", "✓"),
            self.paint("2", &format_duration(self.elapsed()))
        ));
    }

    /// End the command on `line`, replacing the spinner. Nothing under `quiet`.
    pub fn done_with(&self, line: &str) {
        match &self.mode {
            Mode::Quiet => {}
            Mode::Plain => eprintln!("{line}"),
            Mode::Live(_) => {
                self.clear();
                eprintln!("{line}");
            }
        }
    }

    /// End the command with nothing said — the spinner is cleared, the stage lines stay.
    pub fn clear(&self) {
        if let Mode::Live(spinner) = &self.mode
            && let Some(spinner) = spinner.get()
        {
            spinner.finish_and_clear();
        }
    }

    /// Print `diagnostic` on stderr, drawn against `sources` — the spinner cleared first,
    /// so the report opens on a line of its own. A diagnostic prints under `quiet` too.
    pub fn report(&self, diagnostic: &Diagnostic, sources: &SourceMap) {
        self.clear();
        eprintln!("{}", diagnostic.render(sources, self.color));
    }

    /// `text` in ANSI style `code` when color is on, bare otherwise.
    pub fn paint(&self, code: &str, text: &str) -> String {
        match self.color {
            true => format!("\x1b[{code}m{text}\x1b[0m"),
            false => text.to_string(),
        }
    }
}

/// Whether stderr wants color: a terminal, with no `NO_COLOR` or `TERM=dumb` opt-out —
/// the same answer the runtime gives a compiled program.
pub fn color_enabled() -> bool {
    quilon_rt::__color_enabled(2) == 1
}

/// Whether stderr is a terminal a person is watching live: a tty, and `CI` unset — a CI
/// runner often allocates a pty, but there is no one there to watch a spinner animate, so
/// it gets the same plain, scrollback-safe output a pipe does.
fn is_interactive() -> bool {
    std::io::stderr().is_terminal() && !std::env::var_os("CI").is_some_and(|v| !v.is_empty())
}

/// A duration as a reader scans it: whole milliseconds under a second, otherwise seconds
/// to one decimal.
pub fn format_duration(duration: Duration) -> String {
    match duration.as_secs_f64() {
        seconds if seconds < 1.0 => format!("{}ms", duration.as_millis()),
        seconds => format!("{seconds:.1}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_milliseconds_then_seconds() {
        assert_eq!(format_duration(Duration::from_millis(12)), "12ms");
        assert_eq!(format_duration(Duration::from_millis(1234)), "1.2s");
    }

    #[test]
    fn a_silent_status_paints_nothing() {
        assert_eq!(Status::silent().paint("32", "x"), "x");
    }
}
