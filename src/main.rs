mod lexer;
mod parser;
mod ast;
mod typechecker;
mod codegen;
mod runtime;

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
            if let Some(out) = output {
                println!("📦 Output: {}", out.display());
            }
            // TODO: Implement compile
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
                    println!("📋 Program contains {} top-level item(s)", program.items.len());
                }
                Err(e) => {
                    eprintln!("❌ Type error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}

