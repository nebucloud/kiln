// Rust guideline compliant 2026-02-21
//! kiln — command-line interface.
//!
//! Thin dispatcher over [`crate::commands`]. Errors are printed with
//! their `std::error::Error::source()` chain so consumers see the
//! full root cause.

use std::error::Error;
use std::process::ExitCode;

use clap::Parser as _;

mod args;
mod commands;

use crate::args::{CacheCommand, Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kiln: {err}");
            let mut source = err.source();
            while let Some(s) = source {
                eprintln!("  caused by: {s}");
                source = s.source();
            }
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: &Cli) -> Result<(), Box<dyn Error>> {
    match &cli.command {
        Command::Run(args) => commands::run::run(args),
        Command::Validate(args) => commands::validate::run(args),
        Command::Inspect(args) => commands::inspect::run(args),
        Command::Cache(CacheCommand::Stats(args)) => commands::cache::stats(args),
        Command::Cache(CacheCommand::Clear(args)) => commands::cache::clear(args),
    }
}
