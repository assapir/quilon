mod ast;
mod build;
mod codegen;
mod diagnostic;
mod driver;
mod jit;
mod lexer;
mod modules;
mod parser;
mod runtime;
mod typechecker;

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
        /// Path to the .ql file
        file: PathBuf,
        /// Arguments passed through to the program itself (available via `^`'s
        /// `args`). Everything after the file path is forwarded verbatim, so
        /// `quilon run f.ql --flag x` gives the program `[f.ql, --flag, x]` and
        /// behaves like `./f --flag x`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compile a Quilon program
    Compile {
        /// Path to the .ql file
        file: PathBuf,
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Build a Quilon program into a native executable
    Build {
        /// Path to the .ql file
        file: PathBuf,
        /// Output executable path (defaults to the source name without `.ql`)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Linker to drive the final link (clang is natural for LLVM objects)
        #[arg(long, default_value = "clang")]
        linker: String,
    },
    /// Check a Quilon program for errors without running
    Check {
        /// Path to the .ql file
        file: PathBuf,
    },
}

/// Run the shared front-end (read → lex → parse → resolve imports → type-check),
/// printing the diagnostic and exiting on any failure.
fn checked_program(file: &Path) -> ast::Program {
    match driver::front_end(file) {
        Ok(program) => program,
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
            let program = checked_program(&file);
            require_entry_point(&program);

            // Mirror the argv a native build receives: `argv[0]` is the program
            // (here, the `.ql` file path as typed), followed by the user's
            // trailing args. This keeps `quilon run f.ql a b c` and `./f a b c`
            // in agreement on `^`'s `args` — same `args.size` and same trailing
            // arguments (argv[0] is the `.ql` path rather than the binary path).
            // Fixes the JIT leaking the `quilon run` CLI prefix (issue #44).
            let mut argv = Vec::with_capacity(args.len() + 1);
            argv.push(file.to_string_lossy().into_owned());
            argv.extend(args);

            // JIT-compile and execute in-process; the entry point's value
            // becomes the program's exit code.
            match jit::run_program(&program, &argv) {
                Ok(code) => std::process::exit(code),
                Err(e) => {
                    eprintln!("❌ Runtime error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Compile { file, output } => {
            println!("🔨 Compiling: {}", file.display());

            let program = checked_program(&file);
            println!("✅ Type checking passed!");
            require_entry_point(&program);

            // Generate LLVM IR
            use inkwell::context::Context;
            let context = Context::create();
            let mut generator =
                match codegen::CodeGenerator::with_oracle(&context, "main", &program) {
                    Ok(g) => g,
                    Err(e) => {
                        eprintln!("❌ Code generation error: {}", e);
                        std::process::exit(1);
                    }
                };

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
        } => {
            println!("🔨 Building: {}", file.display());

            let program = checked_program(&file);
            require_entry_point(&program);

            // Default the output to the source name without its `.ql` extension.
            let out = output.unwrap_or_else(|| file.with_extension(""));

            match build::build_native(&program, &out, &linker) {
                Ok(()) => println!("✅ Built native executable: {}", out.display()),
                Err(e) => {
                    eprintln!("❌ Build error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { file } => {
            println!("🔍 Checking: {}", file.display());

            let program = checked_program(&file);
            println!("✅ Type checking passed!");
            println!(
                "📋 Program contains {} top-level item(s)",
                program.items.len()
            );
        }
    }
}
