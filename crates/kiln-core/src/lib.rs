// Rust guideline compliant 2026-02-21
//! Core types, planner, and validation for kiln pipelines.
//!
//! kiln-core is the foundation crate of the kiln workspace. It defines
//! the structural primitives every other kiln crate (and downstream
//! consumer) depends on:
//!
//! - [`Pipeline`] / [`Target`] / [`TargetId`] / [`ShellBlock`] —
//!   the unit of execution and the bag-of-targets that contains them.
//! - [`Resource`] / [`ResourceRef`] / [`ResourceId`] / [`AccessMode`] —
//!   shared mutable state targets can declare.
//! - [`KilnType`] — schema-metadata enum for typed inputs/outputs
//!   (currently informational; not enforced at runtime).
//! - [`KilnError`] — the single situation-specific error struct
//!   raised by the planner and validator (per M-ERRORS-CANONICAL-STRUCTS).
//!
//! Pure data and pure algorithms only. No I/O, no sandboxing, no
//! subprocess management — those concerns live in `kiln-cache` (M3) and
//! `kiln-exec` (M4).
//!
//! # Wire format
//!
//! Pipelines round-trip through JSON via the derived
//! [`Serialize`](serde::Serialize) / [`Deserialize`](serde::Deserialize)
//! impls. The targets field serializes as a JSON object keyed by
//! [`TargetId`], matching the wire format documented in
//! [KLN-D-extraction-decisions §KLN-D-02].
//!
//! ```
//! use kiln_core::{Pipeline, Target, TargetId, ShellBlock};
//!
//! let mut pipeline = Pipeline::new();
//! pipeline.add(
//!     TargetId::new("hello"),
//!     Target::new(ShellBlock::new("bash", "echo hello")),
//! );
//!
//! let json = serde_json::to_string(&pipeline).unwrap();
//! let parsed: Pipeline = serde_json::from_str(&json).unwrap();
//! assert_eq!(parsed, pipeline);
//! ```
//!
//! [KLN-D-extraction-decisions §KLN-D-02]: https://github.com/nebucloud/docs/blob/main/KLN-D-extraction-decisions.md

pub mod builder;
pub mod error;
pub mod manifest;
pub mod planner;
pub mod resource;
pub mod target;
pub mod types;
pub mod validator;

#[doc(inline)]
pub use crate::builder::{PipelineBuilder, TargetBuilder};
#[doc(inline)]
pub use crate::error::KilnError;
#[doc(inline)]
pub use crate::planner::{build_execution_plan, ExecutionPlan};
#[doc(inline)]
pub use crate::resource::{AccessMode, Resource, ResourceId, ResourceRef};
#[doc(inline)]
pub use crate::target::{FetchSpec, Pipeline, PipelineVersion, ShellBlock, Target, TargetId};
#[doc(inline)]
pub use crate::types::KilnType;
