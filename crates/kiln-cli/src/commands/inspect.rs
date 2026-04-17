// Rust guideline compliant 2026-02-21
//! `kiln inspect <key> --cache <dir>` — print one cache entry.

use std::error::Error;
use std::path::PathBuf;

use crate::args::InspectArgs;

/// Runs the `kiln inspect` subcommand.
///
/// Locates the entry directory under the
/// [`LocalDiskStore`](kiln_cache::LocalDiskStore) layout
/// (`{root}/{ab}/{cd...}/`), reads `result.json` plus the captured
/// `stdout` and `stderr` blobs, and pretty-prints them.
///
/// # Errors
///
/// - Hex key shorter than 4 characters.
/// - I/O errors reading the entry.
/// - Missing `result.json` (no entry under that key).
pub fn run(args: &InspectArgs) -> Result<(), Box<dyn Error>> {
    let key = args.key.trim();
    if key.len() < 4 {
        return Err(format!("hex key `{key}` is too short (must be at least 4 chars)").into());
    }

    let entry_dir: PathBuf = {
        let (shard, rest) = key.split_at(2);
        args.cache.join(shard).join(rest)
    };

    let result_path = entry_dir.join("result.json");
    if !result_path.exists() {
        return Err(format!(
            "no cache entry for key `{key}` (looked under `{}`)",
            entry_dir.display()
        )
        .into());
    }

    let result_json = std::fs::read_to_string(&result_path)?;
    let stdout = std::fs::read_to_string(entry_dir.join("stdout")).unwrap_or_default();
    let stderr = std::fs::read_to_string(entry_dir.join("stderr")).unwrap_or_default();

    println!("kiln cache entry `{key}`:");
    println!("  path: {}", entry_dir.display());
    println!("  result.json:");
    for line in result_json.lines() {
        println!("    {line}");
    }
    if !stdout.is_empty() {
        println!("  stdout ({} bytes):", stdout.len());
        for line in stdout.lines().take(20) {
            println!("    {line}");
        }
        let extra = stdout.lines().count().saturating_sub(20);
        if extra > 0 {
            println!("    ... ({extra} more line(s))");
        }
    }
    if !stderr.is_empty() {
        println!("  stderr ({} bytes):", stderr.len());
        for line in stderr.lines().take(20) {
            println!("    {line}");
        }
        let extra = stderr.lines().count().saturating_sub(20);
        if extra > 0 {
            println!("    ... ({extra} more line(s))");
        }
    }
    Ok(())
}
