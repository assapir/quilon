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
            // TODO: Implement check
        }
    }
}

