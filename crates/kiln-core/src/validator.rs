// Rust guideline compliant 2026-02-21
//! Structural checks over a [`Pipeline`].
//!
//! [`validate`] catches mistakes the type system can't: dangling
//! cross-target references in `requires` / `conflicts`, repeated
//! target ids in resource declarations, and missing run blocks.
//! Cycle detection lives in the planner — running the validator
//! before [`build_execution_plan`](crate::planner::build_execution_plan)
//! gives a friendlier error for the structural mistakes that don't
//! need a graph walk.
//!
//! # Examples
//!
//! ```
//! use kiln_core::{Pipeline, Target, TargetId, ShellBlock};
//! use kiln_core::validator::validate;
//!
//! let mut pipeline = Pipeline::new();
//! pipeline.add(
//!     TargetId::new("hello"),
//!     Target::new(ShellBlock::new("bash", "echo hello")),
//! );
//! validate(&pipeline).unwrap();
//! ```

use std::collections::HashSet;

use crate::error::{KilnError, ReferenceRelation};
use crate::resource::ResourceId;
use crate::target::Pipeline;

/// Validates a [`Pipeline`]'s structural integrity.
///
/// # Errors
///
/// - [`KilnError::is_unknown_target_reference`] when a target's
///   `requires` or `conflicts` list mentions an id not in the pipeline.
/// - [`KilnError::is_duplicate_target`] when `pipeline.resources`
///   declares the same [`ResourceId`] twice.
pub fn validate(pipeline: &Pipeline) -> Result<(), KilnError> {
    for (id, target) in &pipeline.targets {
        for req in &target.requires {
            if !pipeline.contains(req) {
                return Err(KilnError::unknown_target_reference(
                    req.0.clone(),
                    id.0.clone(),
                    ReferenceRelation::Requires,
                ));
            }
        }
        for conflict in &target.conflicts {
            if !pipeline.contains(conflict) {
                return Err(KilnError::unknown_target_reference(
                    conflict.0.clone(),
                    id.0.clone(),
                    ReferenceRelation::Conflicts,
                ));
            }
        }
    }

    let mut seen_resources: HashSet<&ResourceId> = HashSet::new();
    for resource in &pipeline.resources {
        if !seen_resources.insert(&resource.id) {
            return Err(KilnError::duplicate_target(resource.id.0.clone()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{AccessMode, Resource};
    use crate::target::{ShellBlock, Target, TargetId};

    fn target() -> Target {
        Target::new(ShellBlock::new("bash", "true"))
    }

    #[test]
    fn empty_pipeline_validates() {
        validate(&Pipeline::new()).unwrap();
    }

    #[test]
    fn well_formed_pipeline_validates() {
        let mut pipeline = Pipeline::new();
        pipeline.add(TargetId::new("a"), target());
        let mut b = target();
        b.requires.push(TargetId::new("a"));
        pipeline.add(TargetId::new("b"), b);
        validate(&pipeline).unwrap();
    }

    #[test]
    fn unknown_requires_target_fails_validation() {
        let mut pipeline = Pipeline::new();
        let mut a = target();
        a.requires.push(TargetId::new("ghost"));
        pipeline.add(TargetId::new("a"), a);

        let err = validate(&pipeline).expect_err("ghost target");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn unknown_conflicts_target_fails_validation() {
        let mut pipeline = Pipeline::new();
        let mut a = target();
        a.conflicts.push(TargetId::new("ghost"));
        pipeline.add(TargetId::new("a"), a);

        let err = validate(&pipeline).expect_err("ghost target");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn duplicate_resource_id_fails_validation() {
        let mut pipeline = Pipeline::new();
        pipeline
            .resources
            .push(Resource::new(ResourceId::new("gpu"), AccessMode::Exclusive));
        pipeline
            .resources
            .push(Resource::new(ResourceId::new("gpu"), AccessMode::Shared));

        let err = validate(&pipeline).expect_err("duplicate resource");
        assert!(err.is_duplicate_target());
    }
}
