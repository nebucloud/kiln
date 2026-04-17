// Rust guideline compliant 2026-02-21
//! `kiln cache stats` and `kiln cache clear` subcommands.
//!
//! Reads the on-disk layout produced by
//! [`LocalDiskStore`](kiln_cache::LocalDiskStore): `{root}/{ab}/{cd...}/`.
//! Stats walks the tree counting entries (one per `result.json`) and
//! summing total bytes. Clear removes everything under the cache
//! root after the user confirms with `--yes`.

use std::error::Error;
use std::path::Path;

use crate::args::{CacheClearArgs, CacheStatsArgs};

/// Runs `kiln cache stats`.
///
/// # Errors
///
/// Returns I/O errors from reading the cache directory tree.
pub fn stats(args: &CacheStatsArgs) -> Result<(), Box<dyn Error>> {
    if !args.cache.exists() {
        println!(
            "kiln: cache `{}` does not exist (0 entries).",
            args.cache.display()
        );
        return Ok(());
    }

    let mut entry_count = 0_usize;
    let mut total_bytes = 0_u64;

    walk_cache(&args.cache, &mut |path, size| {
        if path.file_name().and_then(|n| n.to_str()) == Some("result.json") {
            entry_count += 1;
        }
        total_bytes += size;
    })?;

    println!("kiln cache `{}`:", args.cache.display());
    println!("  entries: {entry_count}");
    println!(
        "  total bytes: {total_bytes} ({})",
        human_bytes(total_bytes)
    );
    Ok(())
}

/// Runs `kiln cache clear`.
///
/// # Errors
///
/// Returns I/O errors from removing the cache directory tree.
pub fn clear(args: &CacheClearArgs) -> Result<(), Box<dyn Error>> {
    if !args.yes {
        return Err(format!(
            "kiln cache clear `{}` requires `--yes` to confirm (destructive).",
            args.cache.display()
        )
        .into());
    }

    if !args.cache.exists() {
        println!("kiln: cache `{}` already absent.", args.cache.display());
        return Ok(());
    }

    std::fs::remove_dir_all(&args.cache)?;
    println!("kiln: cleared cache `{}`.", args.cache.display());
    Ok(())
}

/// Recursively walks `dir`, calling `on_file(path, size)` for every regular file.
fn walk_cache<F>(dir: &Path, on_file: &mut F) -> Result<(), Box<dyn Error>>
where
    F: FnMut(&Path, u64),
{
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            walk_cache(&path, on_file)?;
        } else if metadata.is_file() {
            on_file(&path, metadata.len());
        }
    }
    Ok(())
}

/// Renders a byte count in a human-readable form (KiB / MiB / GiB).
#[allow(
    clippy::cast_precision_loss,
    reason = "byte counts up to ~9 PiB fit in f64's 52-bit mantissa without loss; this is \
              human-readable display only — exact bytes are printed alongside in stats()."
)]
fn human_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if n >= GIB {
        format!("{:.2} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.2} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.2} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}
