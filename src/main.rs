mod ast;
mod codegen;
mod lexer;
mod parser;
mod runtime;
mod typechecker;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file } => {
            println!("🚀 Running Quilon program: {}", file.display());
            // TODO: Implement run
        }
        Commands::Compile { file, output } => {
            println!("🔨 Compiling: {}", file.display());

            // Read the file
            let source = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("❌ Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            // Lex
            let tokens = match lexer::Lexer::tokenize(&source) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("❌ Lexer error: {}", e);
                    std::process::exit(1);
                }
            };

            // Parse
            let program = match parser::parse(&tokens) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("❌ Parse error: {}", e);
                    std::process::exit(1);
                }
            };

            // Type check
            let mut checker = typechecker::TypeChecker::new();
            match checker.check_program(&program) {
                Ok(()) => {
                    println!("✅ Type checking passed!");
                }
                Err(e) => {
                    eprintln!("❌ Type error: {}", e);
                    std::process::exit(1);
                }
            }

            // Validate entry point exists (>> function required for executables)
            let has_entry_point = program.items.iter().any(|item| {
                if let ast::Item::FunctionDecl(func) = item {
                    func.name == ">>"
                } else {
                    false
                }
            });

            if !has_entry_point {
                eprintln!("❌ Error: No entry point found!");
                eprintln!("   Programs must define a >> function as the entry point.");
                eprintln!("   Example: >> = () -> Num => 0");
                eprintln!();
                eprintln!("   Note: Module imports (<<) are not yet implemented.");
                eprintln!("   For now, all programs must be standalone executables.");
                std::process::exit(1);
            }

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
                        "   clang {} -o executable",
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

            // Read the file
            let source = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("❌ Error reading file: {}", e);
                    std::process::exit(1);
                }
            };

            // Lex
            let tokens = match lexer::Lexer::tokenize(&source) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("❌ Lexer error: {}", e);
                    std::process::exit(1);
                }
            };

            // Parse
            let program = match parser::parse(&tokens) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("❌ Parse error: {}", e);
                    std::process::exit(1);
                }
            };

            // Type check
            let mut checker = typechecker::TypeChecker::new();
            match checker.check_program(&program) {
                Ok(()) => {
                    println!("✅ Type checking passed!");
                    println!(
                        "📋 Program contains {} top-level item(s)",
                        program.items.len()
                    );
                }
                Err(e) => {
                    eprintln!("❌ Type error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
