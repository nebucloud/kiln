// Rust guideline compliant 2026-02-21
//! Schema types for the named inputs and outputs of a target.
//!
//! [`KilnType`] enumerates the value-shape categories kiln pipelines
//! understand. The set is intentionally narrow — a kiln pipeline only
//! needs to describe the type of each named input/output well enough
//! for human readers and downstream tooling. Real values flow through
//! the pipeline as opaque strings or paths; this enum is metadata.
//!
//! lattice's richer `LatticeType` (with first-class `Constraint` and
//! `LatticeValue` variants) stays in terranox-tools per
//! [KLN-D-extraction-decisions §2.3].
//!
//! [KLN-D-extraction-decisions §2.3]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

use serde::{Deserialize, Serialize};

/// A category describing the shape of a named target input or output value.
///
/// kiln does not perform runtime value checking against this type — it is a
/// documentation surface for pipeline authors and downstream tooling. The
/// values themselves flow through targets as strings the shell interprets.
///
/// # Examples
///
/// ```
/// use kiln_core::KilnType;
///
/// let json = serde_json::to_string(&KilnType::Path).unwrap();
/// assert_eq!(json, "\"path\"");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KilnType {
    /// A filesystem path.
    Path,
    /// A free-form string.
    String,
    /// A signed integer.
    Int,
    /// A boolean.
    Bool,
    /// A byte size (e.g. 16MB). Encoded as a string at the wire layer.
    Size,
    /// A duration (e.g. 5s, 200ms). Encoded as a string at the wire layer.
    Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_uses_snake_case() {
        assert_eq!(serde_json::to_string(&KilnType::Path).unwrap(), "\"path\"");
        assert_eq!(serde_json::to_string(&KilnType::Int).unwrap(), "\"int\"");
        assert_eq!(
            serde_json::to_string(&KilnType::Duration).unwrap(),
            "\"duration\""
        );
    }

    #[test]
    fn round_trips_through_json() {
        for ty in [
            KilnType::Path,
            KilnType::String,
            KilnType::Int,
            KilnType::Bool,
            KilnType::Size,
            KilnType::Duration,
        ] {
            let json = serde_json::to_string(&ty).unwrap();
            let parsed: KilnType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, ty);
        }
    }
}
