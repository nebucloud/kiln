// Rust guideline compliant 2026-02-21
//! clap CLI definition.
//!
//! Single root [`Cli`] with [`Command`] subcommands. Argument
//! structs are public so [`crate::commands`] modules can take them
//! by reference.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Top-level CLI for the `kiln` binary.
#[derive(Parser, Debug)]
#[command(
    name = "kiln",
    version,
    about = "Hermetic, content-addressed task execution.",
    long_about = "kiln runs declarative pipeline manifests. Targets execute inside per-target \
                  sandboxes with a content-addressed cache and an optional fetch-then-seal \
                  network policy. See https://github.com/nebucloud/kiln for the manifest schema."
)]
pub struct Cli {
    /// Subcommand to invoke.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Execute a pipeline manifest end-to-end.
    Run(RunArgs),

    /// Validate a pipeline manifest without executing it.
    Validate(ValidateArgs),

    /// Inspect or maintain the content-addressed cache.
    #[command(subcommand)]
    Cache(CacheCommand),

    /// Print the metadata of a single cache entry by its hex key.
    Inspect(InspectArgs),
}

/// Arguments for `kiln run`.
#[derive(Args, Debug)]
pub struct RunArgs {
    /// Path to the pipeline manifest (JSON).
    pub pipeline: PathBuf,

    /// Cache directory. When omitted, runs without caching.
    #[arg(long, value_name = "DIR")]
    pub cache: Option<PathBuf>,

    /// Disable sandbox isolation entirely (for debugging only).
    #[arg(long)]
    pub no_sandbox: bool,

    /// Seal the network during run blocks (KLN-D-04 policy).
    #[arg(long)]
    pub seal_network: bool,
}

/// Arguments for `kiln validate`.
#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Path to the pipeline manifest (JSON).
    pub pipeline: PathBuf,
}

/// Subcommands of `kiln cache`.
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Report counts and total size of cache entries.
    Stats(CacheStatsArgs),
    /// Remove every entry under the cache directory.
    Clear(CacheClearArgs),
}

/// Arguments for `kiln cache stats`.
#[derive(Args, Debug)]
pub struct CacheStatsArgs {
    /// Cache directory to inspect.
    #[arg(long, value_name = "DIR")]
    pub cache: PathBuf,
}

/// Arguments for `kiln cache clear`.
#[derive(Args, Debug)]
pub struct CacheClearArgs {
    /// Cache directory to clear.
    #[arg(long, value_name = "DIR")]
    pub cache: PathBuf,

    /// Required confirmation flag — `clear` is destructive.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `kiln inspect`.
#[derive(Args, Debug)]
pub struct InspectArgs {
    /// Combined hex cache key (BLAKE3 of input || script || tool).
    pub key: String,

    /// Cache directory to inspect.
    #[arg(long, value_name = "DIR")]
    pub cache: PathBuf,
}
