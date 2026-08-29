//! Repository-maintenance commands.
//!
//! `cargo xtask gen-core` parses vendored Hook headers and regenerates the
//! translated `rshooks-core` sources. This host-side tool permits bounded
//! parser arithmetic and indexing.
#![allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
#![allow(clippy::print_stderr)]

mod codegen;
mod gen_core;
mod ir;
mod parse;
mod protocol_ir;
mod protocol_parse;
mod render;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "rshooks repo-maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate `rshooks-core` sources from vendored Hook headers.
    GenCore {
        /// Check whether generated sources are up to date without writing them.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::GenCore { check: true } => gen_core::run_check(),
        Command::GenCore { check: false } => gen_core::run_update(),
    };
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
