mod ast;
mod codegen;
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
    },
    /// Compile a Quilon program
    Compile {
        /// Path to the .ql file
        file: PathBuf,
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
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
        Commands::Run { file } => {
            let program = checked_program(&file);
            require_entry_point(&program);

            // JIT-compile and execute in-process; the entry point's value
            // becomes the program's exit code.
            match jit::run_program(&program) {
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
            let mut generator = codegen::CodeGenerator::new(&context, "main");

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
                    println!("💡 To compile to native code, run:");
                    println!("   llc -filetype=obj {}", output_path.display());
                    println!(
                        "   gcc {} -o executable",
                        output_path.with_extension("o").display()
                    );
                }
                Err(e) => {
                    eprintln!("❌ Error writing output: {}", e);
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
