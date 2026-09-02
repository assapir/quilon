//! The compiler's voice: one-liners for the status lines, the success line, the test
//! summary, and the banner — never for an error message, which stays precise and
//! searchable. The material is programming culture, Quilon's own identity (no keywords,
//! `^`, `~`, `<<`, backticks, `Ok`/`NotOk`), and the Enderverse the release codenames come
//! from. Dry, short, never at the user's expense.
//!
//! Rotation is deterministic under `QUILON_QUIP_SEED=<n>` — the same `n` picks the same line
//! from every list — and otherwise turns with the clock. `--quiet` prints none of these.

use std::sync::OnceLock;

pub const LEXING: &[&str] = &[
    "lexing — splitting hairs",
    "lexing — every symbol counted, no keywords found",
    "lexing — the enemy's gate is down",
];

pub const PARSING: &[&str] = &[
    "parsing — reading between the lines",
    "parsing — seventeen levels of precedence, one at a time",
    "parsing — reading Locke and Demosthenes",
];

pub const RESOLVING: &[&str] = &[
    "resolving — following every `<<` home",
    "resolving — calling in the jeesh",
    "resolving — the ansible is up",
];

pub const CHECKING: &[&str] = &[
    "checking — trust, but verify",
    "checking — Ok or NotOk, nothing in between",
    "checking — Battle School, final exam",
];

pub const GENERATING: &[&str] = &[
    "generating — explaining ourselves to LLVM",
    "generating — turning intent into instructions",
    "generating — Dragon Army takes the field",
];

pub const LINKING: &[&str] = &[
    "linking — the wire is warm",
    "linking — all the pieces, one piece",
    "linking — the game was real",
];

pub const SUCCESS: &[&str] = &[
    "no keywords were harmed",
    "`^` is the way in",
    "compiled. the enemy's gate was down all along",
    "not a keyword in sight",
    "`~` nothing to add",
    "Ok(binary)",
    "the game was real, and it compiled",
    "the wire is warm and waiting",
    "Bean would have found a shorter way. This one works.",
];

pub const TESTS_PASSED: &[&str] = &[
    "The tests have no notes.",
    "all Ok, no NotOk",
    "every arm matched",
    "the jeesh is intact",
    "Battle School: perfect record",
    "trust, verified",
];

pub const TESTS_FAILED: &[&str] = &[
    "One of these is not like the others.",
    "some notes, then",
    "NotOk — and the game was real",
    "at least one arm fell through",
    "the enemy's gate is that way",
    "expected, meet got",
];

pub const BANNER: &[&str] = &[
    "a compiler with no keywords, looking for a subcommand",
    "speak, friend: `quilon run <file.qn>`",
    "the ansible is listening",
    "no keywords, only symbols",
    "Hegemon is a title, not a threat",
    "one `^` per program; the rest is up to you",
];

/// One line from `list`. The same seed picks the same line from every list, so a run
/// pinned with `QUILON_QUIP_SEED` is reproducible end to end.
pub fn pick(list: &'static [&'static str]) -> &'static str {
    static SEED: OnceLock<u64> = OnceLock::new();
    let seed = SEED.get_or_init(|| {
        std::env::var("QUILON_QUIP_SEED")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos() as u64)
            })
    });
    list[(*seed % list.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_comes_from_its_list_and_is_stable_within_a_run() {
        let first = pick(SUCCESS);
        assert!(SUCCESS.contains(&first));
        assert_eq!(first, pick(SUCCESS));
    }

    #[test]
    fn every_list_is_short_dry_and_not_an_error_message() {
        for list in [
            LEXING,
            PARSING,
            RESOLVING,
            CHECKING,
            GENERATING,
            LINKING,
            SUCCESS,
            TESTS_PASSED,
            TESTS_FAILED,
            BANNER,
        ] {
            assert!(!list.is_empty());
            for quip in list {
                assert!(quip.chars().count() <= 60, "too long: {quip}");
                assert!(
                    !quip.starts_with("error"),
                    "an error is never a quip: {quip}"
                );
            }
        }
    }
}
