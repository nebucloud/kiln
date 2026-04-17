// Rust guideline compliant 2026-02-21
// Ported from terranoxos/terranox-tools/crates/lattice-exec/src/planner.rs
// (commit 7ab5316ac6 baseline). Adapted to take a kiln `Pipeline` rather
// than parallel `&[Target]` / `&[TargetId]` slices, and to drop the
// solver-specific "enabled set" filtering — kiln executes every target
// in the pipeline. See KLN-PLAN-extraction §6 for the provenance
// convention.
//! DAG topological sort and wave extraction.
//!
//! Takes a [`Pipeline`] and produces an [`ExecutionPlan`]: a sequence
//! of waves where targets within each wave are independent and can run
//! in parallel. Waves themselves run sequentially — every target in
//! wave *n* must complete before wave *n+1* begins.
//!
//! The planner is pure data-and-algorithm work; it does no I/O and no
//! sandboxing. It runs Kahn's algorithm on top of [`petgraph`] for the
//! initial level grouping, then a greedy second pass that splits waves
//! when targets either declare a `conflicts` relation or both want
//! [`AccessMode::Exclusive`] access to the same [`Resource`].
//!
//! # Examples
//!
//! ```
//! use kiln_core::{Pipeline, Target, TargetId, ShellBlock};
//! use kiln_core::planner::build_execution_plan;
//!
//! let mut pipeline = Pipeline::new();
//! pipeline.add(TargetId::new("a"), Target::new(ShellBlock::new("bash", "true")));
//! let mut b = Target::new(ShellBlock::new("bash", "true"));
//! b.requires.push(TargetId::new("a"));
//! pipeline.add(TargetId::new("b"), b);
//!
//! let plan = build_execution_plan(&pipeline).unwrap();
//! assert_eq!(plan.waves.len(), 2);
//! assert_eq!(plan.waves[0], vec![TargetId::new("a")]);
//! assert_eq!(plan.waves[1], vec![TargetId::new("b")]);
//! ```
//!
//! [`Resource`]: crate::Resource
//! [`AccessMode::Exclusive`]: crate::AccessMode::Exclusive

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use petgraph::algo::is_cyclic_directed;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::error::{KilnError, ReferenceRelation};
use crate::resource::AccessMode;
use crate::target::{Pipeline, Target, TargetId};

/// An execution plan: targets grouped into sequential waves.
///
/// Targets within a single wave are independent and may run
/// concurrently. Waves themselves are strictly ordered: all targets in
/// wave *n* must complete before wave *n+1* begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    /// Ordered waves of target ids.
    pub waves: Vec<Vec<TargetId>>,
}

/// Builds an [`ExecutionPlan`] from a [`Pipeline`].
///
/// Constructs a dependency DAG over the pipeline's targets, checks for
/// cycles, then extracts waves via Kahn's algorithm. A second pass
/// splits any wave whose members declare a `conflicts` edge with each
/// other or compete for the same exclusive [`Resource`](crate::Resource).
///
/// # Errors
///
/// Returns:
///
/// - [`KilnError::is_cyclic_dependency`] when the dependency graph
///   contains a cycle.
/// - [`KilnError::is_unknown_target_reference`] when a target's
///   `requires` or `conflicts` list mentions an id not in the pipeline.
///
/// # Panics
///
/// Panics on detected programming errors per M-PANIC-ON-BUG: that
/// every petgraph edge has endpoints, that the wave queue is non-empty
/// when popped, and that every node was inserted into the in-degree
/// map. None of these can fire under correct use of this function.
pub fn build_execution_plan(pipeline: &Pipeline) -> Result<ExecutionPlan, KilnError> {
    if pipeline.is_empty() {
        return Ok(ExecutionPlan { waves: vec![] });
    }

    let target_map: &BTreeMap<TargetId, Target> = &pipeline.targets;

    // Validate cross-references before touching petgraph so we surface
    // the friendlier KilnError variant instead of a graph-level error.
    for (id, target) in target_map {
        for req in &target.requires {
            if !target_map.contains_key(req) {
                return Err(KilnError::unknown_target_reference(
                    req.0.clone(),
                    id.0.clone(),
                    ReferenceRelation::Requires,
                ));
            }
        }
        for conflict in &target.conflicts {
            if !target_map.contains_key(conflict) {
                return Err(KilnError::unknown_target_reference(
                    conflict.0.clone(),
                    id.0.clone(),
                    ReferenceRelation::Conflicts,
                ));
            }
        }
    }

    // Build the directed dependency graph. Edge B -> A means "B must
    // run before A" (i.e. A requires B).
    let mut graph: DiGraph<&TargetId, ()> = DiGraph::new();
    let mut id_to_node: HashMap<&TargetId, NodeIndex> = HashMap::with_capacity(target_map.len());

    for id in target_map.keys() {
        let node = graph.add_node(id);
        id_to_node.insert(id, node);
    }

    for (id, target) in target_map {
        let a_node = id_to_node[id];
        for req in &target.requires {
            // The unknown-reference check above guarantees the id is present.
            let b_node = id_to_node[req];
            graph.add_edge(b_node, a_node, ());
        }
    }

    if is_cyclic_directed(&graph) {
        return Err(KilnError::cyclic_dependency(
            "cycle detected in target dependency graph",
        ));
    }

    // Kahn's algorithm with level grouping.
    let mut in_degree: HashMap<NodeIndex, usize> = HashMap::with_capacity(graph.node_count());
    for node in graph.node_indices() {
        in_degree.insert(node, 0);
    }
    for edge in graph.edge_indices() {
        // SAFETY-equivalent: edges always have endpoints; petgraph guarantees this.
        let (_, target) = graph
            .edge_endpoints(edge)
            .expect("edge index from graph must yield endpoints");
        *in_degree.entry(target).or_insert(0) += 1;
    }

    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    for (&node, &degree) in &in_degree {
        if degree == 0 {
            queue.push_back(node);
        }
    }

    let mut waves: Vec<Vec<TargetId>> = Vec::new();

    while !queue.is_empty() {
        let wave_size = queue.len();
        let mut wave: Vec<TargetId> = Vec::with_capacity(wave_size);

        for _ in 0..wave_size {
            let node = queue
                .pop_front()
                .expect("loop bound matches queue length at entry");
            wave.push(graph[node].clone());

            let neighbors: Vec<NodeIndex> = graph.neighbors(node).collect();
            for neighbor in neighbors {
                let degree = in_degree
                    .get_mut(&neighbor)
                    .expect("every node was inserted into in_degree above");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(neighbor);
                }
            }
        }

        wave.sort();
        waves.push(wave);
    }

    let waves = split_conflicting_waves(waves, target_map);

    Ok(ExecutionPlan { waves })
}

/// Splits a wave so conflicting targets, or targets contending for the
/// same exclusive resource, never share a wave.
///
/// A greedy single pass: each target slots into the first sub-wave
/// where it has no conflict with already-placed targets and no
/// resource contention. Sub-waves preserve the dependency ordering
/// because all sub-waves derived from wave *n* still complete before
/// wave *n+1* begins.
fn split_conflicting_waves(
    waves: Vec<Vec<TargetId>>,
    targets: &BTreeMap<TargetId, Target>,
) -> Vec<Vec<TargetId>> {
    let mut result: Vec<Vec<TargetId>> = Vec::new();

    for wave in waves {
        let mut sub_waves: Vec<Vec<TargetId>> = Vec::new();
        let mut sub_wave_members: Vec<HashSet<&TargetId>> = Vec::new();
        let mut sub_wave_holds: Vec<HashMap<&str, bool>> = Vec::new();

        for target_id in &wave {
            let Some(target) = targets.get(target_id) else {
                // Should never happen — validate_references guards entry.
                if sub_waves.is_empty() {
                    sub_waves.push(Vec::new());
                    sub_wave_members.push(HashSet::new());
                    sub_wave_holds.push(HashMap::new());
                }
                sub_waves[0].push(target_id.clone());
                continue;
            };

            let mut placed = false;
            for index in 0..sub_waves.len() {
                if has_direct_conflict(target, &sub_wave_members[index]) {
                    continue;
                }
                if has_resource_contention(target, &sub_wave_holds[index]) {
                    continue;
                }
                sub_waves[index].push(target_id.clone());
                sub_wave_members[index].insert(target_id);
                add_resource_holds(target, &mut sub_wave_holds[index]);
                placed = true;
                break;
            }

            if !placed {
                let mut new_members: HashSet<&TargetId> = HashSet::new();
                new_members.insert(target_id);
                let mut new_holds: HashMap<&str, bool> = HashMap::new();
                add_resource_holds(target, &mut new_holds);
                sub_waves.push(vec![target_id.clone()]);
                sub_wave_members.push(new_members);
                sub_wave_holds.push(new_holds);
            }
        }

        for sub_wave in &mut sub_waves {
            sub_wave.sort();
        }
        result.extend(sub_waves);
    }

    result
}

fn has_direct_conflict(target: &Target, members: &HashSet<&TargetId>) -> bool {
    target.conflicts.iter().any(|c| members.contains(c))
}

fn has_resource_contention(target: &Target, holds: &HashMap<&str, bool>) -> bool {
    target.resources.iter().any(|res_ref| {
        holds
            .get(res_ref.resource_id.0.as_str())
            .is_some_and(|&held_exclusive| {
                held_exclusive || res_ref.access == AccessMode::Exclusive
            })
    })
}

fn add_resource_holds<'a>(target: &'a Target, holds: &mut HashMap<&'a str, bool>) {
    for res_ref in &target.resources {
        let is_exclusive = res_ref.access == AccessMode::Exclusive;
        holds
            .entry(res_ref.resource_id.0.as_str())
            .and_modify(|e| *e = *e || is_exclusive)
            .or_insert(is_exclusive);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ResourceId, ResourceRef};
    use crate::target::ShellBlock;

    fn target() -> Target {
        Target::new(ShellBlock::new("bash", "true"))
    }

    fn pipeline_from(targets: Vec<(&str, Target)>) -> Pipeline {
        let mut pipeline = Pipeline::new();
        for (id, target) in targets {
            pipeline.add(TargetId::new(id), target);
        }
        pipeline
    }

    #[test]
    fn empty_pipeline_yields_empty_plan() {
        let plan = build_execution_plan(&Pipeline::new()).unwrap();
        assert!(plan.waves.is_empty());
    }

    #[test]
    fn single_target_runs_alone() {
        let pipeline = pipeline_from(vec![("a", target())]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves, vec![vec![TargetId::new("a")]]);
    }

    #[test]
    fn linear_chain_produces_sequential_waves() {
        let mut a = target();
        a.requires.push(TargetId::new("b"));
        let mut b = target();
        b.requires.push(TargetId::new("c"));
        let c = target();

        let pipeline = pipeline_from(vec![("a", a), ("b", b), ("c", c)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(
            plan.waves,
            vec![
                vec![TargetId::new("c")],
                vec![TargetId::new("b")],
                vec![TargetId::new("a")],
            ]
        );
    }

    #[test]
    fn independent_targets_share_a_wave() {
        let pipeline = pipeline_from(vec![("a", target()), ("b", target()), ("c", target())]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 1);
        assert_eq!(
            plan.waves[0],
            vec![TargetId::new("a"), TargetId::new("b"), TargetId::new("c")]
        );
    }

    #[test]
    fn diamond_dependency_three_waves() {
        let a = target();
        let mut b = target();
        b.requires.push(TargetId::new("a"));
        let mut c = target();
        c.requires.push(TargetId::new("a"));
        let mut d = target();
        d.requires.push(TargetId::new("b"));
        d.requires.push(TargetId::new("c"));

        let pipeline = pipeline_from(vec![("a", a), ("b", b), ("c", c), ("d", d)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 3);
        assert_eq!(plan.waves[0], vec![TargetId::new("a")]);
        assert_eq!(plan.waves[1], vec![TargetId::new("b"), TargetId::new("c")]);
        assert_eq!(plan.waves[2], vec![TargetId::new("d")]);
    }

    #[test]
    fn cycle_returns_cyclic_dependency_error() {
        let mut a = target();
        a.requires.push(TargetId::new("b"));
        let mut b = target();
        b.requires.push(TargetId::new("a"));

        let pipeline = pipeline_from(vec![("a", a), ("b", b)]);
        let err = build_execution_plan(&pipeline).expect_err("cycle must surface");
        assert!(err.is_cyclic_dependency());
    }

    #[test]
    fn unknown_requires_target_surfaces_typed_error() {
        let mut a = target();
        a.requires.push(TargetId::new("nope"));
        let pipeline = pipeline_from(vec![("a", a)]);
        let err = build_execution_plan(&pipeline).expect_err("unknown reference");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn unknown_conflicts_target_surfaces_typed_error() {
        let mut a = target();
        a.conflicts.push(TargetId::new("nope"));
        let pipeline = pipeline_from(vec![("a", a)]);
        let err = build_execution_plan(&pipeline).expect_err("unknown reference");
        assert!(err.is_unknown_target_reference());
    }

    #[test]
    fn conflicting_targets_split_into_separate_waves() {
        let mut a = target();
        a.conflicts.push(TargetId::new("b"));
        let mut b = target();
        b.conflicts.push(TargetId::new("a"));

        let pipeline = pipeline_from(vec![("a", a), ("b", b)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 2);
    }

    #[test]
    fn three_way_conflict_packs_compatible_pair() {
        // a conflicts b, b conflicts c. a + c don't conflict directly,
        // so they can pack into wave 0 and b takes wave 1.
        let mut a = target();
        a.conflicts.push(TargetId::new("b"));
        let mut b = target();
        b.conflicts.push(TargetId::new("a"));
        b.conflicts.push(TargetId::new("c"));
        let mut c = target();
        c.conflicts.push(TargetId::new("b"));

        let pipeline = pipeline_from(vec![("a", a), ("b", b), ("c", c)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 2);
        assert_eq!(plan.waves[0], vec![TargetId::new("a"), TargetId::new("c")]);
        assert_eq!(plan.waves[1], vec![TargetId::new("b")]);
    }

    #[test]
    fn exclusive_resource_holders_split_wave() {
        let mut a = target();
        a.resources.push(ResourceRef::new(
            ResourceId::new("gpu"),
            AccessMode::Exclusive,
        ));
        let mut b = target();
        b.resources.push(ResourceRef::new(
            ResourceId::new("gpu"),
            AccessMode::Exclusive,
        ));

        let pipeline = pipeline_from(vec![("a", a), ("b", b)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 2);
    }

    #[test]
    fn shared_resource_holders_share_a_wave() {
        let mut a = target();
        a.resources.push(ResourceRef::new(
            ResourceId::new("network"),
            AccessMode::Shared,
        ));
        let mut b = target();
        b.resources.push(ResourceRef::new(
            ResourceId::new("network"),
            AccessMode::Shared,
        ));

        let pipeline = pipeline_from(vec![("a", a), ("b", b)]);
        let plan = build_execution_plan(&pipeline).unwrap();
        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].len(), 2);
    }
}
