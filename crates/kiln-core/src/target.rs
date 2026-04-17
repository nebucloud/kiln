// Rust guideline compliant 2026-02-21
//! Targets, shell blocks, and pipelines — kiln's structural primitives.
//!
//! A [`Target`] is the unit of execution: an interpreter and a script to
//! run, plus declarative metadata about what it depends on, conflicts
//! with, and which shared [`Resource`]s it uses. A [`Pipeline`] is the
//! collection of targets the executor schedules together.
//!
//! Compared to the lattice-exec types these were ported from, the kiln
//! versions drop two TerranoxOS-specific fields (per
//! [KLN-D-extraction-decisions §2.3]):
//!
//! - `Target.constraints: Vec<Constraint>` — the SAT-solver coupling
//!   stays in lattice-exec, which wraps `kiln::Target` in its own
//!   `LatticeTarget` type.
//! - `Target.inputs`/`outputs: HashMap<String, LatticeType>` — kiln's
//!   inputs/outputs are simple `Vec<String>` name lists. The richer
//!   [`KilnType`](crate::KilnType) enum is available as documentation
//!   metadata but is not part of the [`Target`] schema.
//!
//! [`Resource`]: crate::Resource
//! [KLN-D-extraction-decisions §2.3]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::resource::{Resource, ResourceRef};

/// Schema version of a serialized [`Pipeline`] manifest.
///
/// Pre-1.0 the only accepted value is `V1` (wire form `"1"`). New
/// versions will be added as the JSON shape evolves; existing kiln
/// releases will reject unknown versions on deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PipelineVersion {
    /// Pipeline schema version 1.
    #[default]
    #[serde(rename = "1")]
    V1,
}

impl Display for PipelineVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1 => f.write_str("1"),
        }
    }
}

/// A globally-unique identifier for a [`Target`] within a [`Pipeline`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetId(pub String);

impl TargetId {
    /// Constructs a `TargetId` from any string-like value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl Display for TargetId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl AsRef<str> for TargetId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for TargetId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TargetId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// One declared resource a target needs fetched before its run block.
///
/// Fetches run in the unsealed phase of the sandbox (network still
/// accessible) per [KLN-D-04][kln-d-04]: kiln pulls each fetch, writes
/// it into the sandbox workspace at `destination`, optionally
/// verifying the BLAKE3 digest, and then seals the network before
/// invoking the target's `run` block.
///
/// # 0.1.0 limitation
///
/// Only BLAKE3 verification is supported in 0.1.0. SHA-256 (the
/// algorithm the KLN-D-extraction-decisions §02 example uses) lands
/// in 0.2 — register the fetch with `blake3_hex = None` until then if
/// the upstream only publishes a SHA-256 digest.
///
/// [kln-d-04]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchSpec {
    /// URL of the resource to fetch.
    pub url: String,
    /// Path inside the sandbox workspace where the fetched bytes are
    /// written. Relative paths are resolved against the workspace
    /// root.
    pub destination: std::path::PathBuf,
    /// Optional lowercase-hex BLAKE3 digest of the response body.
    ///
    /// When `Some`, the [`Fetcher`](https://docs.rs/kiln-exec/latest/kiln_exec/trait.Fetcher.html)
    /// must reject the response on mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3_hex: Option<String>,
}

impl FetchSpec {
    /// Constructs a `FetchSpec` from a URL and destination.
    #[must_use]
    pub fn new(url: impl Into<String>, destination: impl Into<std::path::PathBuf>) -> Self {
        Self {
            url: url.into(),
            destination: destination.into(),
            blake3_hex: None,
        }
    }

    /// Returns this `FetchSpec` with the supplied BLAKE3 hex digest attached.
    #[must_use]
    pub fn with_blake3(mut self, hex: impl Into<String>) -> Self {
        self.blake3_hex = Some(hex.into());
        self
    }
}

/// An interpreter name plus the script source the interpreter executes.
///
/// kiln dispatches a [`Target`]'s `run` (and optional `cleanup`)
/// [`ShellBlock`]s by writing the `code` to a temp file and invoking
/// the named `interpreter` against it. The interpreter must be on
/// `PATH` inside the sandbox (kiln-exec, M4) or pre-staged via the
/// fetch-then-seal flow (KLN-D-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellBlock {
    /// Interpreter name, e.g. `bash`, `zsh`, `python3`, `pwsh`.
    pub interpreter: String,
    /// Script source the interpreter executes.
    pub code: String,
}

impl ShellBlock {
    /// Constructs a new `ShellBlock`.
    #[must_use]
    pub fn new(interpreter: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            interpreter: interpreter.into(),
            code: code.into(),
        }
    }
}

impl Display for ShellBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "shell:{} ({} bytes)", self.interpreter, self.code.len())
    }
}

/// One unit of execution in a [`Pipeline`].
///
/// `requires` and `conflicts` reference other targets by id within the
/// same pipeline. `resources` declares which shared [`Resource`]s this
/// target needs and how it accesses them — the planner uses this to
/// keep targets with overlapping exclusive holds in separate waves.
///
/// `inputs` and `outputs` are name lists for human readers and for
/// content-addressed cache key composition; kiln does not type-check
/// values at runtime.
///
/// # Examples
///
/// ```
/// use kiln_core::{Target, ShellBlock};
///
/// let target = Target::new(ShellBlock::new("bash", "echo hello"));
/// assert_eq!(target.run.interpreter, "bash");
/// assert!(target.requires.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The main execution block.
    pub run: ShellBlock,

    /// Optional cleanup block. kiln runs cleanup on target failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<ShellBlock>,

    /// Other target ids that must complete successfully before this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<TargetId>,

    /// Other target ids that must not co-execute with this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<TargetId>,

    /// Shared resources this target uses, with access modes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceRef>,

    /// Named inputs (e.g. `artifact_ref`). For documentation and cache keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,

    /// Named outputs (e.g. `signature`). For documentation and cache keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,

    /// Resources to fetch into the sandbox before the network is sealed.
    ///
    /// kiln runs each entry's fetch while the sandbox network is still
    /// open, writes the bytes to the workspace at the spec's
    /// `destination`, optionally verifies its BLAKE3 digest, and only
    /// then seals the network and invokes the run block. Implements
    /// the fetch-then-seal pattern from KLN-D-04.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fetches: Vec<FetchSpec>,

    /// Arbitrary metadata for downstream tooling.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Target {
    /// Constructs a new `Target` with only the `run` block populated.
    ///
    /// All other fields start empty / `None`. For richer construction use
    /// the builder API in the `kiln_core::builder` module (lands later in M2).
    #[must_use]
    pub fn new(run: ShellBlock) -> Self {
        Self {
            run,
            cleanup: None,
            requires: Vec::new(),
            conflicts: Vec::new(),
            resources: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            fetches: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

/// A complete kiln pipeline — the unit the planner and executor consume.
///
/// A pipeline is a versioned bag of targets keyed by id, plus optional
/// shared [`Resource`] declarations and arbitrary metadata. Pipelines
/// are the canonical wire format (per [KLN-D-02]) and round-trip
/// through JSON via the derived [`Serialize`]/[`Deserialize`] impls.
///
/// # Examples
///
/// ```
/// use kiln_core::{Pipeline, Target, TargetId, ShellBlock};
///
/// let mut pipeline = Pipeline::new();
/// pipeline.add(
///     TargetId::new("hello"),
///     Target::new(ShellBlock::new("bash", "echo hello")),
/// );
///
/// assert!(pipeline.contains(&TargetId::new("hello")));
/// assert_eq!(pipeline.len(), 1);
/// ```
///
/// [KLN-D-02]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Pipeline {
    /// Schema version of this pipeline.
    #[serde(default)]
    pub version: PipelineVersion,

    /// Targets keyed by id. `BTreeMap` for deterministic iteration order
    /// (important for content-addressed cache keys later).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<TargetId, Target>,

    /// Shared resources declared at pipeline scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,

    /// Arbitrary metadata for downstream tooling.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Pipeline {
    /// Constructs an empty pipeline at the current schema version.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a target into the pipeline under `id`.
    ///
    /// Replaces any existing target with the same id and returns the
    /// previous value. Use [`Pipeline::contains`] beforehand if you need
    /// to detect duplicates without overwriting.
    pub fn add(&mut self, id: TargetId, target: Target) -> Option<Target> {
        self.targets.insert(id, target)
    }

    /// Returns the target registered under `id`, if any.
    #[must_use]
    pub fn target(&self, id: &TargetId) -> Option<&Target> {
        self.targets.get(id)
    }

    /// Returns `true` if `id` refers to a target in the pipeline.
    #[must_use]
    pub fn contains(&self, id: &TargetId) -> bool {
        self.targets.contains_key(id)
    }

    /// Returns the number of targets in the pipeline.
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// Returns `true` if the pipeline has no targets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Runs structural validation over the pipeline.
    ///
    /// Thin wrapper around [`validator::validate`](crate::validator::validate).
    ///
    /// # Errors
    ///
    /// See [`validator::validate`](crate::validator::validate) for the
    /// exact set of [`KilnError`](crate::KilnError) variants this can
    /// return.
    pub fn validate(&self) -> Result<(), crate::KilnError> {
        crate::validator::validate(self)
    }

    /// Returns a fresh [`PipelineBuilder`](crate::builder::PipelineBuilder).
    ///
    /// See `kiln_core::builder` for the chainable API.
    #[must_use]
    pub fn builder() -> crate::builder::PipelineBuilder {
        crate::builder::PipelineBuilder::new()
    }

    /// Parses a [`Pipeline`] from a JSON string and validates it.
    ///
    /// Thin wrapper around
    /// [`manifest::from_json_str`](crate::manifest::from_json_str).
    ///
    /// # Errors
    ///
    /// See [`manifest::from_json_str`](crate::manifest::from_json_str).
    pub fn from_json_str(json: &str) -> Result<Self, crate::KilnError> {
        crate::manifest::from_json_str(json)
    }

    /// Serializes this pipeline to a compact JSON string.
    ///
    /// # Errors
    ///
    /// See
    /// [`manifest::to_json_string`](crate::manifest::to_json_string).
    pub fn to_json_string(&self) -> Result<String, crate::KilnError> {
        crate::manifest::to_json_string(self)
    }

    /// Serializes this pipeline to a pretty-printed JSON string.
    ///
    /// # Errors
    ///
    /// See
    /// [`manifest::to_json_string_pretty`](crate::manifest::to_json_string_pretty).
    pub fn to_json_string_pretty(&self) -> Result<String, crate::KilnError> {
        crate::manifest::to_json_string_pretty(self)
    }
}

impl Target {
    /// Returns a fresh [`TargetBuilder`](crate::builder::TargetBuilder).
    ///
    /// See `kiln_core::builder` for the chainable API.
    #[must_use]
    pub fn builder() -> crate::builder::TargetBuilder {
        crate::builder::TargetBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_with(interpreter: &str, code: &str) -> Target {
        Target::new(ShellBlock::new(interpreter, code))
    }

    #[test]
    fn target_id_from_str_and_string() {
        let from_str: TargetId = "build".into();
        let from_string: TargetId = String::from("build").into();
        assert_eq!(from_str, from_string);
        assert_eq!(from_str.as_ref(), "build");
        assert_eq!(from_str.to_string(), "build");
    }

    #[test]
    fn target_id_serializes_as_plain_string() {
        let id = TargetId::new("hello");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"hello\"");
        let parsed: TargetId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn shell_block_display_lists_byte_count() {
        let block = ShellBlock::new("python3", "print('hi')");
        assert_eq!(block.to_string(), "shell:python3 (11 bytes)");
    }

    #[test]
    fn target_default_fields_are_empty() {
        let target = target_with("bash", "true");
        assert_eq!(target.run.interpreter, "bash");
        assert!(target.cleanup.is_none());
        assert!(target.requires.is_empty());
        assert!(target.conflicts.is_empty());
        assert!(target.resources.is_empty());
        assert!(target.inputs.is_empty());
        assert!(target.outputs.is_empty());
        assert!(target.metadata.is_empty());
    }

    #[test]
    fn pipeline_add_and_lookup() {
        let mut pipeline = Pipeline::new();
        assert!(pipeline.is_empty());

        let previous = pipeline.add(TargetId::new("a"), target_with("bash", "true"));
        assert!(previous.is_none());

        assert_eq!(pipeline.len(), 1);
        assert!(pipeline.contains(&TargetId::new("a")));
        assert!(!pipeline.contains(&TargetId::new("b")));
        assert!(pipeline.target(&TargetId::new("a")).is_some());
    }

    #[test]
    fn pipeline_add_replaces_existing_target() {
        let mut pipeline = Pipeline::new();
        pipeline.add(TargetId::new("a"), target_with("bash", "first"));
        let previous = pipeline.add(TargetId::new("a"), target_with("bash", "second"));
        let prev = previous.expect("first insert returned");
        assert_eq!(prev.run.code, "first");

        let current = pipeline.target(&TargetId::new("a")).unwrap();
        assert_eq!(current.run.code, "second");
    }

    #[test]
    fn pipeline_round_trips_through_json_with_target_map() {
        let mut pipeline = Pipeline::new();
        let mut sign = target_with("bash", "cosign sign $ARTIFACT");
        sign.requires.push(TargetId::new("build"));
        sign.inputs.push("artifact_ref".to_owned());
        sign.outputs.push("signature".to_owned());
        pipeline.add(TargetId::new("sign"), sign);

        let json = serde_json::to_string(&pipeline).unwrap();
        // The targets field must be a JSON object keyed by target name,
        // not an array — the wire format from KLN-D-02.
        assert!(
            json.contains(r#""targets":{"sign":{"#),
            "expected map-keyed targets in JSON: {json}"
        );
        // The version must serialize as "1".
        assert!(json.contains(r#""version":"1""#), "{json}");

        let parsed: Pipeline = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, pipeline);
    }

    #[test]
    fn pipeline_version_serializes_as_string_one() {
        let json = serde_json::to_string(&PipelineVersion::V1).unwrap();
        assert_eq!(json, "\"1\"");
    }

    #[test]
    fn types_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<Pipeline>();
        assert_send::<Target>();
        assert_send::<TargetId>();
        assert_send::<ShellBlock>();
        assert_send::<PipelineVersion>();
    }
}
