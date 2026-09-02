use quilon::diagnostic::{Code, Diagnostic, codes};
use quilon::source_map::SourceMap;
use quilon::status::{Stage, Status};
use quilon::{build, codegen, driver, jit, quips, test_command};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "quilon")]
#[command(about = "Quilon - A fast, statically-typed web programming language", long_about = None)]
#[command(arg_required_else_help = true)]
struct Cli {
    /// Print no status — diagnostics still print
    #[arg(short, long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a Quilon program
    Run {
        /// Path to the .qn file
        file: PathBuf,
        /// Arguments passed through to the program itself (available via `^`'s
        /// `args`). Everything after the file path is forwarded verbatim, so
        /// `quilon run f.qn --flag x` gives the program `[f.qn, --flag, x]` and
        /// behaves like `./f --flag x`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compile a Quilon program
    Compile {
        /// Path to the .qn file
        file: PathBuf,
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Build a Quilon program into a native executable
    Build {
        /// Path to the .qn file
        file: PathBuf,
        /// Output executable path (defaults to the source name without `.qn`)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Linker to drive the final link (clang is natural for LLVM objects)
        #[arg(long, default_value = "clang")]
        linker: String,
        /// Emit DWARF line-number debug info (source-level debugging: gdb/lldb line
        /// stepping, backtraces referencing `.qn` lines). Builds are already unoptimized.
        #[arg(short = 'g', long)]
        debug: bool,
    },
    /// Check a Quilon program for errors without running
    Check {
        /// Path to the .qn file
        file: PathBuf,
    },
    /// Run a Quilon test suite: every top-level `describe` block in a file or directory
    Test {
        /// File or directory to search for suites (defaults to the current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Explain an error code (`quilon explain Q028`)
    Explain {
        /// The code, as a report prints it
        code: String,
    },
}

/// The release codenames, matched against the package version.
const CODENAMES: &str = include_str!("../release-codenames.tsv");

/// `quilon 0.9.3 "Hegemon"` — the version with its codename, when the release table
/// names one.
fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let codename = CODENAMES
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .find(|(pattern, _)| match pattern.strip_suffix('*') {
            Some(prefix) => version.starts_with(prefix),
            None => *pattern == version,
        })
        .map(|(_, name)| name.trim());
    match codename {
        Some(name) => format!("{version} \"{name}\""),
        None => version.to_string(),
    }
}

/// Print `diagnostic` the way every report is printed, and exit 1.
fn fail(diagnostic: &Diagnostic, sources: &SourceMap, status: &Status) -> ! {
    eprintln!("{}", diagnostic.render(sources, status.color()));
    std::process::exit(1)
}

/// Run the shared front-end (read → lex → parse → resolve imports → type-check),
/// printing the diagnostic and exiting on any failure. The result carries the type
/// table the check produced, which codegen consumes instead of checking again.
fn checked(file: &Path, status: &Status) -> driver::Checked {
    match driver::front_end_reporting(file, driver::TestBlocks::Erase, status) {
        Ok(checked) => checked,
        Err(error) => fail(&error.diagnostic, &error.sources, status),
    }
}

/// [`checked`], exiting 0 without a word when the file is a test suite rather than a
/// program. Erasing its blocks leaves nothing to run, so `run`, `compile`, and `build`
/// pass over it rather than reporting a missing `^`; `quilon test` is what runs it. Call it
/// before printing anything, so the skip is silent.
fn checked_program_to_emit(file: &Path, status: &Status) -> driver::Checked {
    let checked = checked(file, status);
    if checked.tests_only {
        std::process::exit(0);
    }
    if !driver::has_entry_point(&checked.program) {
        let diagnostic = Diagnostic::new(
            Code::NoEntryPoint,
            format!("`{}` defines no `^` entry point", file.display()),
        )
        .help("a program starts at `^`: `^ = () -> Num => < 0 >`");
        fail(&diagnostic, &checked.sources, status);
    }
    checked
}

/// A failure after the front end — code generation, the link, the JIT — which has no
/// source location of its own.
fn fail_late(code: Code, message: String, status: &Status) -> ! {
    fail(
        &Diagnostic::new(code, message),
        &SourceMap::default(),
        status,
    )
}

fn main() {
    let command = Cli::command()
        .version(&*version().leak())
        .after_help(quips::pick(quips::BANNER));
    let cli = Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit());
    let status = Status::for_command(cli.quiet);

    match cli.command {
        Commands::Run { file, args } => {
            let checked = checked_program_to_emit(&file, &status);

            // Mirror the argv a native build receives: `argv[0]` is the program
            // (here, the `.qn` file path as typed), followed by the user's
            // trailing args. This keeps `quilon run f.qn a b c` and `./f a b c`
            // in agreement on `^`'s `args` — same `args.size` and same trailing
            // arguments (argv[0] is the `.qn` path rather than the binary path).
            // Keeps the JIT from leaking the `quilon run` CLI prefix.
            let mut argv = Vec::with_capacity(args.len() + 1);
            argv.push(file.to_string_lossy().into_owned());
            argv.extend(args);

            // The program's own output stands alone: the spinner is gone before it runs,
            // and nothing is said after it.
            status.stage(Stage::Generating);
            status.clear();
            match jit::run_program(
                &checked.program,
                checked.types,
                checked.defer,
                checked.sources,
                &argv,
            ) {
                Ok(code) => std::process::exit(code),
                Err(e) => fail_late(Code::CodegenFailed, e, &status),
            }
        }
        Commands::Compile { file, output } => {
            let checked = checked_program_to_emit(&file, &status);
            let program = checked.program;

            status.stage(Stage::Generating);
            use inkwell::context::Context;
            let context = Context::create();
            let mut generator = codegen::CodeGenerator::new(&context, "main");
            generator.set_type_table(checked.types);
            generator.set_defer_info(checked.defer);
            // `quilon compile` emits the IR an ahead-of-time build would.
            generator.set_aot();
            generator.set_source_map(checked.sources);

            let ir = match generator.generate(&program) {
                Ok(ir) => ir,
                Err(e) => fail_late(Code::CodegenFailed, e, &status),
            };

            let output_path = output.unwrap_or_else(|| {
                let mut path = file.clone();
                path.set_extension("ll");
                path
            });

            if let Err(e) = std::fs::write(&output_path, ir) {
                fail_late(
                    Code::BuildFailed,
                    format!("cannot write `{}`: {e}", output_path.display()),
                    &status,
                );
            }
            status.done(
                &format!("{} → {}", file.display(), output_path.display()),
                quips::pick(quips::SUCCESS),
            );
        }
        Commands::Build {
            file,
            output,
            linker,
            debug,
        } => {
            // The source text and every file's path come from the source map the build already
            // carries; a `--debug` build additionally needs the root file's path (below).
            let checked = checked_program_to_emit(&file, &status);
            let sources = checked.sources;
            let defer = checked.defer;
            let program = checked.program;

            // Default the output to the source name without its `.qn` extension.
            let out = output.unwrap_or_else(|| file.with_extension(""));

            let debug_source = debug.then(|| build::DebugSource { file: &file });

            if let Err(e) = build::build_native(
                &program,
                checked.types,
                defer,
                sources,
                &out,
                &linker,
                debug_source.as_ref(),
                &status,
            ) {
                fail_late(Code::BuildFailed, e, &status);
            }
            status.done(
                &format!("{} → {}", file.display(), out.display()),
                quips::pick(quips::SUCCESS),
            );
        }
        Commands::Check { file } => {
            checked(&file, &status);
            status.done(&file.display().to_string(), quips::pick(quips::SUCCESS));
        }
        Commands::Test { path } => {
            let failed = test_command::run(&path, cli.quiet);
            std::process::exit(i32::from(failed > 0));
        }
        Commands::Explain { code } => {
            let Some(code) = Code::parse(&code) else {
                eprintln!(
                    "no error code `{code}` — codes run Q000 to {}",
                    codes::ALL[codes::ALL.len() - 1]
                );
                std::process::exit(2);
            };
            match codes::explain(code) {
                Some(section) => println!("{section}"),
                None => {
                    eprintln!("{code} ({}) has no explanation yet", code.title());
                    std::process::exit(2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn version_flags_print_the_package_version_and_codename() {
        for flag in ["--version", "-V"] {
            let error = Cli::command()
                .version(&*version().leak())
                .try_get_matches_from(["quilon", flag])
                .expect_err("the version flags should exit before requiring a subcommand");

            assert_eq!(error.kind(), ErrorKind::DisplayVersion);
            assert_eq!(error.exit_code(), 0);
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("quilon {}", env!("CARGO_PKG_VERSION"))),
                "{error}"
            );
        }
    }

    #[test]
    fn the_codename_comes_from_the_release_table() {
        let expected = CODENAMES
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find(|(pattern, _)| *pattern == env!("CARGO_PKG_VERSION"))
            .map(|(_, name)| name.trim());
        if let Some(name) = expected {
            assert_eq!(
                version(),
                format!("{} \"{name}\"", env!("CARGO_PKG_VERSION"))
            );
        }
    }

    #[test]
    fn no_arguments_shows_the_help() {
        let error = Cli::command()
            .try_get_matches_from(["quilon"])
            .expect_err("no subcommand is a help exit");
        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
