use quilon::{ast, build, codegen, driver, jit, test_command};

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "quilon")]
#[command(about = "Quilon - A fast, statically-typed web programming language", long_about = None)]
#[command(version)]
struct Cli {
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
}

/// Run the shared front-end (read → lex → parse → resolve imports → type-check),
/// printing the diagnostic and exiting on any failure. The result carries the type
/// table the check produced, which codegen consumes instead of checking again.
fn checked(file: &Path) -> driver::Checked {
    match driver::front_end(file) {
        Ok(checked) => checked,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

/// [`checked`], exiting 0 without a word when the file is a test suite rather than a
/// program. Erasing its blocks leaves nothing to run, so `run`, `compile`, and `build`
/// pass over it rather than reporting a missing `^`; `quilon test` is what runs it. Call it
/// before printing anything, so the skip is silent.
fn checked_program_to_emit(file: &Path) -> driver::Checked {
    let checked = checked(file);
    if checked.tests_only {
        std::process::exit(0);
    }
    checked
}

/// Exit with the standard diagnostic unless `program` defines the `^` entry point
/// required to build an executable (compile/run, but not check).
fn require_entry_point(program: &ast::Program) {
    if !driver::has_entry_point(program) {
        eprintln!("❌ Error: No entry point found!");
        eprintln!("   Programs must define a ^ function as the entry point.");
        eprintln!("   Example: ^ = () -> Num => 0");
        std::process::exit(1);
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file, args } => {
            let checked = checked_program_to_emit(&file);
            require_entry_point(&checked.program);

            // Mirror the argv a native build receives: `argv[0]` is the program
            // (here, the `.qn` file path as typed), followed by the user's
            // trailing args. This keeps `quilon run f.qn a b c` and `./f a b c`
            // in agreement on `^`'s `args` — same `args.size` and same trailing
            // arguments (argv[0] is the `.qn` path rather than the binary path).
            // Keeps the JIT from leaking the `quilon run` CLI prefix.
            let mut argv = Vec::with_capacity(args.len() + 1);
            argv.push(file.to_string_lossy().into_owned());
            argv.extend(args);

            // JIT-compile and execute in-process; the entry point's value
            // becomes the program's exit code.
            match jit::run_program(
                &checked.program,
                checked.types,
                checked.defer,
                checked.sources,
                &argv,
            ) {
                Ok(code) => std::process::exit(code),
                Err(e) => {
                    eprintln!("❌ Runtime error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Compile { file, output } => {
            let checked = checked_program_to_emit(&file);
            eprintln!("🔨 Compiling: {}", file.display());
            let program = checked.program;
            eprintln!("✅ Type checking passed!");
            require_entry_point(&program);

            // Generate LLVM IR
            use inkwell::context::Context;
            let context = Context::create();
            let mut generator = codegen::CodeGenerator::new(&context, "main");
            generator.set_type_table(checked.types);
            generator.set_defer_info(checked.defer);
            generator.set_source_map(checked.sources);

            let ir = match generator.generate(&program) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("❌ Code generation error: {}", e);
                    std::process::exit(1);
                }
            };

            // Determine output path
            let output_path = output.unwrap_or_else(|| {
                let mut path = file.clone();
                path.set_extension("ll");
                path
            });

            // Write IR to file
            match std::fs::write(&output_path, ir) {
                Ok(()) => {
                    eprintln!("✅ LLVM IR written to: {}", output_path.display());
                    eprintln!(
                        "💡 To build a native executable directly, run: quilon build {}",
                        file.display()
                    );
                }
                Err(e) => {
                    eprintln!("❌ Error writing output: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Build {
            file,
            output,
            linker,
            debug,
        } => {
            // The source text and every file's path come from the source map the build already
            // carries; a `--debug` build additionally needs the root file's path (below).
            let checked = checked_program_to_emit(&file);
            eprintln!("🔨 Building: {}", file.display());
            let sources = checked.sources;
            let defer = checked.defer;
            let program = checked.program;
            require_entry_point(&program);

            // Default the output to the source name without its `.qn` extension.
            let out = output.unwrap_or_else(|| file.with_extension(""));

            let debug_source = debug.then(|| build::DebugSource { file: &file });

            match build::build_native(
                &program,
                checked.types,
                defer,
                sources,
                &out,
                &linker,
                debug_source.as_ref(),
            ) {
                Ok(()) => eprintln!("✅ Built native executable: {}", out.display()),
                Err(e) => {
                    eprintln!("❌ Build error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { file } => {
            eprintln!("🔍 Checking: {}", file.display());

            let program = checked(&file).program;
            eprintln!("✅ Type checking passed!");
            eprintln!(
                "📋 Program contains {} top-level item(s)",
                program.items.len()
            );
        }
        Commands::Test { path } => {
            let failed = test_command::run(&path);
            std::process::exit(i32::from(failed > 0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::{CommandFactory, error::ErrorKind};

    #[test]
    fn version_flags_print_the_package_version() {
        for flag in ["--version", "-V"] {
            let error = Cli::command()
                .try_get_matches_from(["quilon", flag])
                .expect_err("the version flags should exit before requiring a subcommand");

            assert_eq!(error.kind(), ErrorKind::DisplayVersion);
            assert_eq!(error.exit_code(), 0);
            assert_eq!(
                error.to_string(),
                format!("quilon {}\n", env!("CARGO_PKG_VERSION"))
            );
        }
    }
}
