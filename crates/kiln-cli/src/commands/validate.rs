// Rust guideline compliant 2026-02-21
//! `kiln validate <pipeline.json>` — structural check without execution.

use std::error::Error;

use kiln_core::Pipeline;

use crate::args::ValidateArgs;

/// Runs the `kiln validate` subcommand.
///
/// Loads the manifest, parses it via
/// [`Pipeline::from_json_str`](kiln_core::Pipeline::from_json_str)
/// (which validates structure on load), then prints a one-line summary.
///
/// # Errors
///
/// Returns the I/O error from reading the file or the
/// [`KilnError`](kiln_core::KilnError) from parse / validation.
pub fn run(args: &ValidateArgs) -> Result<(), Box<dyn Error>> {
    let manifest = std::fs::read_to_string(&args.pipeline)?;
    let pipeline = Pipeline::from_json_str(&manifest)?;

    println!(
        "kiln: `{}` valid — {} target(s), {} resource(s).",
        args.pipeline.display(),
        pipeline.len(),
        pipeline.resources.len(),
    );
    Ok(())
}
