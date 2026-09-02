//! What a command says about its own progress, on stderr.
//!
//! On a terminal the stages (lexing, parsing, …) run through one live spinner that clears
//! itself, and a successful command ends on a single line: the file, the elapsed time, and
//! a quip. Without a terminal each stage is one short line. `--quiet` prints nothing here —
//! diagnostics still print, they are not status. `quilon run` clears the spinner and says
//! nothing more, so the program's own output stands alone.

use std::io::IsTerminal;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};

use crate::quips;

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
    /// One line per stage — stderr is not a terminal.
    Lines,
    /// A live spinner that collapses to the final line.
    Live(ProgressBar),
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

    /// The status a command reports on stderr: silent under `quiet`, live on a terminal,
    /// one line per stage otherwise.
    pub fn for_command(quiet: bool) -> Self {
        Self::new(quiet, Mode::Lines)
    }

    /// The status of a command whose output is the program's own (`run`): live on a
    /// terminal, where the spinner leaves no trace, and silent everywhere else.
    pub fn transient(quiet: bool) -> Self {
        Self::new(quiet, Mode::Quiet)
    }

    fn new(quiet: bool, without_terminal: Mode) -> Self {
        let mode = match (quiet, std::io::stderr().is_terminal()) {
            (true, _) => Mode::Quiet,
            (false, false) => without_terminal,
            (false, true) => {
                let spinner = ProgressBar::new_spinner().with_style(
                    ProgressStyle::with_template("{spinner:.cyan} {msg}")
                        .expect("a fixed template"),
                );
                spinner.enable_steady_tick(Duration::from_millis(80));
                Mode::Live(spinner)
            }
        };
        Self {
            mode,
            color: color_enabled(),
            started: Instant::now(),
        }
    }

    /// Whether reports to stderr may carry color.
    pub fn color(&self) -> bool {
        self.color
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Announce that `stage` begins.
    pub fn stage(&self, stage: Stage) {
        match &self.mode {
            Mode::Quiet => {}
            Mode::Lines => eprintln!("{}", stage.line()),
            Mode::Live(spinner) => spinner.set_message(stage.line()),
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
            Mode::Lines => eprintln!("{line}"),
            Mode::Live(spinner) => {
                spinner.finish_and_clear();
                eprintln!("{line}");
            }
        }
    }

    /// End the command with nothing said — the spinner is cleared, the stage lines stay.
    pub fn clear(&self) {
        if let Mode::Live(spinner) = &self.mode {
            spinner.finish_and_clear();
        }
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
