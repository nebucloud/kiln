// Rust guideline compliant 2026-02-21
//! Fluent builders for [`Pipeline`] and [`Target`].
//!
//! Direct field-level construction works for one-off cases, but real
//! pipelines have enough optional state that a chainable builder is
//! easier to read and harder to misuse. KLN-D-extraction-decisions §02
//! makes the builder API the *primary* programmatic surface — the JSON
//! manifest is a serializable mirror of what the builders produce.
//!
//! Builders follow M-INIT-BUILDER:
//!
//! - The buildable type carries a shortcut: [`Pipeline::builder`] /
//!   [`Target::builder`] (defined in `target.rs` to avoid a circular
//!   module dependency).
//! - Builder constructors are crate-private; users start from the
//!   shortcuts.
//! - Methods are chainable and named after the field they set
//!   (`shell`, not `set_shell`).
//! - `.build()` is the terminator; for [`TargetBuilder`] it can fail
//!   with [`KilnError::is_missing_required`] when `shell`/`run` are
//!   absent.
//!
//! # Examples
//!
//! ```
//! use kiln_core::{Pipeline, Target, TargetId};
//!
//! let pipeline = Pipeline::builder()
//!     .target(
//!         "build",
//!         Target::builder()
//!             .shell("bash")
//!             .run("cargo build --release")
//!             .outputs(["target/release/app"])
//!             .build()
//!             .unwrap(),
//!     )
//!     .target(
//!         "sign",
//!         Target::builder()
//!             .shell("bash")
//!             .run("cosign sign $ARTIFACT")
//!             .requires(["build"])
//!             .inputs(["artifact_ref"])
//!             .outputs(["signature"])
//!             .build()
//!             .unwrap(),
//!     )
//!     .build()
//!     .unwrap();
//!
//! assert_eq!(pipeline.len(), 2);
//! assert!(pipeline.contains(&TargetId::new("build")));
//! ```
//!
//! [`Pipeline::builder`]: crate::Pipeline::builder
//! [`Target::builder`]: crate::Target::builder

use std::collections::HashMap;

use crate::error::KilnError;
use crate::resource::{Resource, ResourceRef};
use crate::target::{FetchSpec, Pipeline, PipelineVersion, ShellBlock, Target, TargetId};

/// Fluent builder for [`Target`].
///
/// Construct via [`Target::builder`]. The two required fields are
/// [`Self::shell`] (interpreter name) and [`Self::run`] (script
/// source); calling [`Self::build`] before both are set returns a
/// [`KilnError::is_missing_required`] error.
#[derive(Debug, Clone, Default)]
pub struct TargetBuilder {
    interpreter: Option<String>,
    code: Option<String>,
    cleanup: Option<ShellBlock>,
    requires: Vec<TargetId>,
    conflicts: Vec<TargetId>,
    resources: Vec<ResourceRef>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    fetches: Vec<FetchSpec>,
    metadata: HashMap<String, serde_json::Value>,
}

impl TargetBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Sets the interpreter name (e.g. `bash`, `python3`).
    #[must_use]
    pub fn shell(mut self, interpreter: impl Into<String>) -> Self {
        self.interpreter = Some(interpreter.into());
        self
    }

    /// Sets the script source the interpreter executes.
    #[must_use]
    pub fn run(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Sets the optional cleanup [`ShellBlock`] (runs on target failure).
    #[must_use]
    pub fn cleanup(mut self, cleanup: ShellBlock) -> Self {
        self.cleanup = Some(cleanup);
        self
    }

    /// Replaces the `requires` list with the supplied target ids.
    #[must_use]
    pub fn requires<I, T>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<TargetId>,
    {
        self.requires = items.into_iter().map(Into::into).collect();
        self
    }

    /// Appends one entry to the `requires` list.
    #[must_use]
    pub fn require(mut self, id: impl Into<TargetId>) -> Self {
        self.requires.push(id.into());
        self
    }

    /// Replaces the `conflicts` list with the supplied target ids.
    #[must_use]
    pub fn conflicts<I, T>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<TargetId>,
    {
        self.conflicts = items.into_iter().map(Into::into).collect();
        self
    }

    /// Appends one entry to the `conflicts` list.
    #[must_use]
    pub fn conflict(mut self, id: impl Into<TargetId>) -> Self {
        self.conflicts.push(id.into());
        self
    }

    /// Replaces the `resources` list with the supplied references.
    #[must_use]
    pub fn resources<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = ResourceRef>,
    {
        self.resources = items.into_iter().collect();
        self
    }

    /// Appends one entry to the `resources` list.
    #[must_use]
    pub fn resource(mut self, resource: ResourceRef) -> Self {
        self.resources.push(resource);
        self
    }

    /// Replaces the `inputs` name list.
    #[must_use]
    pub fn inputs<I, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs = items.into_iter().map(Into::into).collect();
        self
    }

    /// Appends one name to the `inputs` list.
    #[must_use]
    pub fn input(mut self, name: impl Into<String>) -> Self {
        self.inputs.push(name.into());
        self
    }

    /// Replaces the `outputs` name list.
    #[must_use]
    pub fn outputs<I, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.outputs = items.into_iter().map(Into::into).collect();
        self
    }

    /// Appends one name to the `outputs` list.
    #[must_use]
    pub fn output(mut self, name: impl Into<String>) -> Self {
        self.outputs.push(name.into());
        self
    }

    /// Replaces the `fetches` list with the supplied specs.
    #[must_use]
    pub fn fetches<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = FetchSpec>,
    {
        self.fetches = items.into_iter().collect();
        self
    }

    /// Appends a [`FetchSpec`] to the target's fetch declarations.
    ///
    /// Convenience constructor: pass URL + destination, no digest. Use
    /// [`Self::fetch_with_blake3`] when you have a verified digest.
    #[must_use]
    pub fn fetch(
        mut self,
        url: impl Into<String>,
        destination: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.fetches.push(FetchSpec::new(url, destination));
        self
    }

    /// Appends a [`FetchSpec`] with a BLAKE3 hex digest attached.
    #[must_use]
    pub fn fetch_with_blake3(
        mut self,
        url: impl Into<String>,
        destination: impl Into<std::path::PathBuf>,
        blake3_hex: impl Into<String>,
    ) -> Self {
        self.fetches
            .push(FetchSpec::new(url, destination).with_blake3(blake3_hex));
        self
    }

    /// Inserts a metadata entry, overwriting any prior value at `key`.
    #[must_use]
    pub fn metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Finalizes the builder into a [`Target`].
    ///
    /// # Errors
    ///
    /// Returns [`KilnError::is_missing_required`] when either `shell`
    /// or `run` is unset.
    pub fn build(self) -> Result<Target, KilnError> {
        let interpreter = self
            .interpreter
            .ok_or_else(|| KilnError::missing_required("shell"))?;
        let code = self
            .code
            .ok_or_else(|| KilnError::missing_required("run"))?;

        Ok(Target {
            run: ShellBlock::new(interpreter, code),
            cleanup: self.cleanup,
            requires: self.requires,
            conflicts: self.conflicts,
            resources: self.resources,
            inputs: self.inputs,
            outputs: self.outputs,
            fetches: self.fetches,
            metadata: self.metadata,
        })
    }
}

/// Fluent builder for [`Pipeline`].
///
/// Construct via [`Pipeline::builder`]. Adds targets, resources, and
/// metadata; [`Self::build`] runs structural validation
/// ([`Pipeline::validate`]) and returns the assembled pipeline.
#[derive(Debug, Clone, Default)]
pub struct PipelineBuilder {
    pipeline: Pipeline,
}

impl PipelineBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Sets the schema version.
    #[must_use]
    pub fn version(mut self, version: PipelineVersion) -> Self {
        self.pipeline.version = version;
        self
    }

    /// Inserts a target into the pipeline under `id`. Replaces any
    /// existing target at the same id.
    #[must_use]
    pub fn target(mut self, id: impl Into<TargetId>, target: Target) -> Self {
        self.pipeline.add(id.into(), target);
        self
    }

    /// Appends a [`Resource`] declaration.
    #[must_use]
    pub fn resource(mut self, resource: Resource) -> Self {
        self.pipeline.resources.push(resource);
        self
    }

    /// Inserts a metadata entry, overwriting any prior value at `key`.
    #[must_use]
    pub fn metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.pipeline.metadata.insert(key.into(), value);
        self
    }

    /// Finalizes the builder into a [`Pipeline`] after structural validation.
    ///
    /// # Errors
    ///
    /// Returns any [`KilnError`] surfaced by [`Pipeline::validate`].
    pub fn build(self) -> Result<Pipeline, KilnError> {
        self.pipeline.validate()?;
        Ok(self.pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{AccessMode, ResourceId};

    #[test]
    fn target_builder_requires_shell_and_run() {
        let err = TargetBuilder::new()
            .build()
            .expect_err("shell missing must error");
        assert!(err.is_missing_required());

        let err = TargetBuilder::new()
            .shell("bash")
            .build()
            .expect_err("run missing must error");
        assert!(err.is_missing_required());

        let err = TargetBuilder::new()
            .run("true")
            .build()
            .expect_err("shell missing must error");
        assert!(err.is_missing_required());
    }

    #[test]
    fn target_builder_populates_all_fields() {
        let target = Target::builder()
            .shell("bash")
            .run("echo hello")
            .cleanup(ShellBlock::new("bash", "echo bye"))
            .requires(["a", "b"])
            .conflicts(["c"])
            .resources([ResourceRef::new(
                ResourceId::new("gpu"),
                AccessMode::Exclusive,
            )])
            .inputs(["src", "dst"])
            .outputs(["log"])
            .metadata("key", serde_json::Value::String("value".to_owned()))
            .build()
            .unwrap();

        assert_eq!(target.run.interpreter, "bash");
        assert_eq!(target.run.code, "echo hello");
        assert!(target.cleanup.is_some());
        assert_eq!(target.requires.len(), 2);
        assert_eq!(target.conflicts.len(), 1);
        assert_eq!(target.resources.len(), 1);
        assert_eq!(target.inputs, vec!["src".to_owned(), "dst".to_owned()]);
        assert_eq!(target.outputs, vec!["log".to_owned()]);
        assert_eq!(
            target.metadata.get("key"),
            Some(&serde_json::Value::String("value".to_owned()))
        );
    }

    #[test]
    fn target_builder_singular_setters_append() {
        let target = Target::builder()
            .shell("bash")
            .run("true")
            .require("a")
            .require("b")
            .conflict("c")
            .input("x")
            .output("y")
            .build()
            .unwrap();

        assert_eq!(
            target.requires,
            vec![TargetId::new("a"), TargetId::new("b")]
        );
        assert_eq!(target.conflicts, vec![TargetId::new("c")]);
        assert_eq!(target.inputs, vec!["x".to_owned()]);
        assert_eq!(target.outputs, vec!["y".to_owned()]);
    }

    #[test]
    fn pipeline_builder_assembles_validated_pipeline() {
        let pipeline = Pipeline::builder()
            .target(
                "a",
                Target::builder().shell("bash").run("true").build().unwrap(),
            )
            .target(
                "b",
                Target::builder()
                    .shell("bash")
                    .run("true")
                    .require("a")
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        assert_eq!(pipeline.len(), 2);
        assert!(pipeline.contains(&TargetId::new("a")));
        assert!(pipeline.contains(&TargetId::new("b")));
    }

    #[test]
    fn pipeline_builder_rejects_unknown_reference() {
        let err = Pipeline::builder()
            .target(
                "a",
                Target::builder()
                    .shell("bash")
                    .run("true")
                    .require("ghost")
                    .build()
                    .unwrap(),
            )
            .build()
            .expect_err("ghost target");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn pipeline_builder_chains_resource_and_metadata() {
        let pipeline = Pipeline::builder()
            .resource(Resource::new(ResourceId::new("gpu"), AccessMode::Exclusive))
            .metadata("owner", serde_json::Value::String("kiln".to_owned()))
            .build()
            .unwrap();

        assert_eq!(pipeline.resources.len(), 1);
        assert_eq!(
            pipeline.metadata.get("owner"),
            Some(&serde_json::Value::String("kiln".to_owned()))
        );
    }

    #[test]
    fn builders_are_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<TargetBuilder>();
        assert_send::<PipelineBuilder>();
    }
}
