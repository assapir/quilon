use quilon::{ast, build, codegen, driver, jit};

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "quilon")]
#[command(about = "Quilon - A fast, statically-typed web programming language", long_about = None)]
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
            let checked = checked(&file);
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
            println!("🔨 Compiling: {}", file.display());

            let checked = checked(&file);
            let program = checked.program;
            println!("✅ Type checking passed!");
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
                    println!("✅ LLVM IR written to: {}", output_path.display());
                    println!(
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
            println!("🔨 Building: {}", file.display());

            // A `--debug` build also needs the import boundary, so only the user's own
            // functions get DWARF line info; the source text comes from the source map the
            // build already carries.
            let checked = checked(&file);
            let sources = checked.sources;
            let debug_imported_items = debug.then_some(checked.imported_items);
            let defer = checked.defer;
            let program = checked.program;
            require_entry_point(&program);

            // Default the output to the source name without its `.qn` extension.
            let out = output.unwrap_or_else(|| file.with_extension(""));

            let debug_source = debug_imported_items.map(|imported_items| build::DebugSource {
                file: &file,
                imported_items,
            });

            match build::build_native(
                &program,
                checked.types,
                defer,
                sources,
                &out,
                &linker,
                debug_source.as_ref(),
            ) {
                Ok(()) => println!("✅ Built native executable: {}", out.display()),
                Err(e) => {
                    eprintln!("❌ Build error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { file } => {
            println!("🔍 Checking: {}", file.display());

            let program = checked(&file).program;
            println!("✅ Type checking passed!");
            println!(
                "📋 Program contains {} top-level item(s)",
                program.items.len()
            );
        }
    }
}
