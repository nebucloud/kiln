// Rust guideline compliant 2026-02-21
//! `kiln run <pipeline.json>` — execute a pipeline manifest.

use std::error::Error;

use kiln_cache::{LocalDiskStore, ShardedCache};
use kiln_core::Pipeline;
use kiln_exec::{Executor, MockFetcher, SandboxConfig};

use crate::args::RunArgs;

/// Runs the `kiln run` subcommand.
///
/// Reads the manifest at `args.pipeline`, builds an [`Executor`] with
/// the requested sandbox config + optional cache, executes the
/// pipeline, and prints a summary to stdout.
///
/// # Errors
///
/// Any error reading or executing the pipeline is returned to the
/// caller for unified rendering.
pub fn run(args: &RunArgs) -> Result<(), Box<dyn Error>> {
    let manifest = std::fs::read_to_string(&args.pipeline)?;
    let pipeline = Pipeline::from_json_str(&manifest)?;

    let sandbox_config = if args.no_sandbox {
        SandboxConfig::default()
    } else if args.seal_network {
        SandboxConfig::enabled()
    } else {
        SandboxConfig {
            enabled: true,
            ..SandboxConfig::default()
        }
    };

    let mut executor = Executor::new(sandbox_config).with_fetcher(MockFetcher::new());
    if let Some(cache_dir) = &args.cache {
        executor = executor.with_cache(ShardedCache::new(LocalDiskStore::new(cache_dir)));
    }

    println!(
        "kiln: running `{}` with {} target(s)",
        args.pipeline.display(),
        pipeline.len()
    );

    let report = executor.execute(&pipeline)?;

    for (wave_index, wave) in report.waves.iter().enumerate() {
        println!("\n  wave {wave_index}:");
        for target_id in wave {
            let result = report
                .results
                .get(target_id)
                .ok_or("internal: missing result for planned target")?;
            let badge = if result.cache_hit {
                "CACHE HIT"
            } else {
                "  RAN   "
            };
            println!("    [{badge}] {target_id} (exit {})", result.exit_code);
            if !result.stdout.is_empty() {
                for line in result.stdout.lines().take(3) {
                    println!("       | {line}");
                }
                let extra = result.stdout.lines().count().saturating_sub(3);
                if extra > 0 {
                    println!("       | ... ({extra} more line(s))");
                }
            }
        }
    }

    println!(
        "\nkiln: done. {} target(s) executed, {} cache hit(s), {} cache miss(es).",
        report.target_count(),
        report.cache_hits(),
        report.cache_misses(),
    );

    Ok(())
}
