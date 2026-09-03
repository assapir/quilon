//! `quilon` with no arguments, and `quilon --version`, each close on a quip from the
//! banner category (see `src/quips.rs::BANNER`) — the same voice every other status line
//! uses, deterministic under `QUILON_QUIP_SEED`.

use std::process::Command;

/// Run the real binary with `args` and `seed` (`QUILON_QUIP_SEED`, when given), returning
/// its exit code, stdout, and stderr.
fn run(args: &[&str], seed: Option<&str>) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command.args(args);
    if let Some(seed) = seed {
        command.env("QUILON_QUIP_SEED", seed);
    } else {
        command.env_remove("QUILON_QUIP_SEED");
    }
    let out = command.output().expect("run quilon");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A line from `src/quips.rs::BANNER`, verbatim (no stage prefix, unlike the stage lists).
const A_BANNER_QUIP: &str = "a compiler with no keywords, looking for a subcommand";

#[test]
fn no_arguments_closes_on_a_banner_quip() {
    // A missing subcommand is a usage error, so clap's help for it goes to stderr — the
    // same stream `--help`'s error-path variant always uses.
    let (code, _stdout, stderr) = run(&[], Some("0"));
    assert_eq!(code, 2, "no subcommand is a usage error");
    assert!(
        stderr.contains("Usage: quilon"),
        "still shows the usage help: {stderr}"
    );
    assert_eq!(
        stderr.lines().last(),
        Some(A_BANNER_QUIP),
        "the last line is a banner quip: {stderr}"
    );
}

#[test]
fn a_different_seed_picks_a_different_banner_quip() {
    let (_, _, first) = run(&[], Some("0"));
    let (_, _, second) = run(&[], Some("1"));
    assert_ne!(
        first.lines().last(),
        second.lines().last(),
        "two seeds landing on the same quip would not prove the seed is read"
    );
}

#[test]
fn version_pairs_the_codename_with_a_quip_on_its_own_line() {
    // `--version` is a successful query, not an error — clap prints it to stdout.
    let (code, stdout, _stderr) = run(&["--version"], Some("0"));
    assert_eq!(code, 0);
    let mut lines = stdout.lines();
    let first = lines.next().expect("a version line");
    assert!(
        first.starts_with(&format!("quilon {}", env!("CARGO_PKG_VERSION"))),
        "{first}"
    );
    let second = lines.next().expect("a quip line under the version");
    assert!(!second.is_empty(), "the quip line is not blank");
    assert_eq!(lines.next(), None, "version is exactly two lines");
}

#[test]
fn quiet_suppresses_the_banner_quip_before_the_subcommand() {
    // `--quiet` with no subcommand still fails to parse (a subcommand is required), and
    // clap's short usage error for that carries no `after_help` banner — quiet in spirit,
    // if not through the `Status` this pass never reaches.
    let (code, _stdout, stderr) = run(&["--quiet"], None);
    assert_ne!(code, 0);
    assert!(
        !stderr.lines().any(|line| line == A_BANNER_QUIP),
        "no quip line under --quiet: {stderr}"
    );
}
