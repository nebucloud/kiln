// Rust guideline compliant 2026-02-21
//! JSON convenience helpers for [`Pipeline`].
//!
//! [`Pipeline`] already derives [`Serialize`](serde::Serialize) and
//! [`Deserialize`](serde::Deserialize), so callers can reach for
//! `serde_json` directly. This module wraps those calls to:
//!
//! - Convert `serde_json::Error` into [`KilnError::is_json_parse_error`]
//!   so consumers don't need to depend on `serde_json` themselves to
//!   handle parse failures.
//! - Validate the parsed [`Pipeline`] structurally on the read path
//!   ([`from_json_str`] / [`from_json_reader`]). Bad JSON shape and
//!   invalid pipeline structure both become [`KilnError`]s, distinguishable
//!   via the `is_*` query methods.
//! - Offer pretty / compact serialization via dedicated functions
//!   rather than caller-side option juggling.
//!
//! The functions live as free functions here and are also exposed as
//! inherent methods on [`Pipeline`] (in `target.rs`) so users can
//! reach them via `Pipeline::from_json_str(...)`.

use std::io::{Read, Write};

use crate::error::KilnError;
use crate::target::Pipeline;

/// Parses a [`Pipeline`] from a JSON string and validates it structurally.
///
/// # Errors
///
/// - [`KilnError::is_json_parse_error`] when the JSON is malformed or
///   does not match the [`Pipeline`] schema.
/// - Any [`KilnError`] surfaced by [`Pipeline::validate`].
pub fn from_json_str(json: &str) -> Result<Pipeline, KilnError> {
    let pipeline: Pipeline = serde_json::from_str(json).map_err(KilnError::json_parse)?;
    pipeline.validate()?;
    Ok(pipeline)
}

/// Parses a [`Pipeline`] from a [`Read`]er and validates it structurally.
///
/// # Errors
///
/// - [`KilnError::is_json_parse_error`] when the input is malformed,
///   does not match the [`Pipeline`] schema, or the underlying reader
///   fails (`serde_json` propagates I/O errors as parse errors).
/// - Any [`KilnError`] surfaced by [`Pipeline::validate`].
pub fn from_json_reader<R: Read>(reader: R) -> Result<Pipeline, KilnError> {
    let pipeline: Pipeline = serde_json::from_reader(reader).map_err(KilnError::json_parse)?;
    pipeline.validate()?;
    Ok(pipeline)
}

/// Serializes a [`Pipeline`] to a compact JSON string.
///
/// # Errors
///
/// Returns [`KilnError::is_json_parse_error`] when serialization
/// fails — for [`Pipeline`] this only happens if a metadata
/// `serde_json::Value` contains a non-finite float.
pub fn to_json_string(pipeline: &Pipeline) -> Result<String, KilnError> {
    serde_json::to_string(pipeline).map_err(KilnError::json_parse)
}

/// Serializes a [`Pipeline`] to a pretty-printed JSON string.
///
/// # Errors
///
/// Returns [`KilnError::is_json_parse_error`] when serialization
/// fails (see [`to_json_string`] for when that can happen).
pub fn to_json_string_pretty(pipeline: &Pipeline) -> Result<String, KilnError> {
    serde_json::to_string_pretty(pipeline).map_err(KilnError::json_parse)
}

/// Serializes a [`Pipeline`] as compact JSON to a [`Write`]r.
///
/// # Errors
///
/// Returns [`KilnError::is_json_parse_error`] when serialization or
/// the underlying writer fails.
pub fn to_json_writer<W: Write>(pipeline: &Pipeline, writer: W) -> Result<(), KilnError> {
    serde_json::to_writer(writer, pipeline).map_err(KilnError::json_parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::{ShellBlock, Target, TargetId};

    fn sample_pipeline() -> Pipeline {
        let mut pipeline = Pipeline::new();
        pipeline.add(
            TargetId::new("hello"),
            Target::new(ShellBlock::new("bash", "echo hello")),
        );
        pipeline
    }

    #[test]
    fn round_trip_through_string() {
        let original = sample_pipeline();
        let json = to_json_string(&original).unwrap();
        let parsed = from_json_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn pretty_string_contains_newlines() {
        let json = to_json_string_pretty(&sample_pipeline()).unwrap();
        assert!(json.contains('\n'), "pretty JSON should contain newlines");
    }

    #[test]
    fn round_trip_through_reader_writer() {
        let original = sample_pipeline();
        let mut buffer = Vec::new();
        to_json_writer(&original, &mut buffer).unwrap();
        let parsed = from_json_reader(buffer.as_slice()).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = from_json_str("not valid json").expect_err("malformed");
        assert!(err.is_json_parse_error());
    }

    #[test]
    fn well_formed_json_with_invalid_structure_is_validation_error() {
        let json = r#"{
            "version": "1",
            "targets": {
                "a": {
                    "run": { "interpreter": "bash", "code": "true" },
                    "requires": ["ghost"]
                }
            }
        }"#;
        let err = from_json_str(json).expect_err("ghost target");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn unknown_pipeline_version_is_parse_error() {
        let json = r#"{ "version": "999", "targets": {} }"#;
        let err = from_json_str(json).expect_err("unsupported version");
        assert!(err.is_json_parse_error());
    }
}
