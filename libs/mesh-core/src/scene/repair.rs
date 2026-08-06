/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide repair driver: runs the per-mesh repair policy over
//! every stored mesh (the [`Scene::mesh_targets`] walk) and reports a
//! per-target outcome, with no global abort. Each target is assessed
//! through a shared borrow first; storage shared through `Arc`s is
//! forked only for meshes that actually receive work, so clean and
//! refused targets keep their pointer identity (callers may diff on
//! it to learn what changed).

use crate::indexed_mesh::repair::{RepairAssessment, assess_repair};
use crate::indexed_mesh::{IndexedMesh, MeshFix, RepairPasses, RepairRefusal};
use crate::scene::Scene;
use crate::scene::mesh_targets::MeshTarget;

/// Everything one repair run did, and did not do, to one stored mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneRepairOutcome {
    /// The storage place this outcome describes.
    pub target: MeshTarget,
    /// Fixes actually applied, in pass order. Empty for clean and
    /// refused targets; can be non-empty alongside a pass-level
    /// refusal (earlier passes' fixes stand).
    pub fixes: Vec<MeshFix>,
    /// Why the run stopped early for this target, if it did.
    pub refusal: Option<RepairRefusal>,
    /// Whether the target's mesh is edge-manifold AFTER the run: the
    /// witness-mint attempt the health sweep also uses. `true` does
    /// not mean "repaired" (a clean mesh reports `true` too), and
    /// `refusal: None` does not mean `true` (holes are not
    /// repairable and not a refusal).
    pub edge_manifold_after: bool,
}

impl Scene {
    /// Run the enabled repair passes over every stored mesh and
    /// report one outcome per target, in [`Scene::mesh_targets`]
    /// order. A refused target never blocks the others, and at every
    /// observable point each mesh is at least as valid as before the
    /// call (refused meshes are byte-identical, applied passes only
    /// shrink the problem set).
    pub fn repair_meshes(&mut self, passes: RepairPasses) -> Vec<SceneRepairOutcome> {
        let targets = self.mesh_targets();
        let mut outcomes = Vec::with_capacity(targets.len());
        for target in targets {
            let Some(mesh) = self.target_mesh(target) else {
                // `mesh_targets` lists live storage only; a dead
                // target here would be a graph invariant break.
                // Skipping keeps the driver total.
                continue;
            };
            let outcome = match assess_repair(mesh, passes) {
                RepairAssessment::Clean => SceneRepairOutcome {
                    target,
                    fixes: Vec::new(),
                    refusal: None,
                    edge_manifold_after: mesh_is_edge_manifold(mesh),
                },
                RepairAssessment::Refused(refusal) => SceneRepairOutcome {
                    target,
                    fixes: Vec::new(),
                    refusal: Some(refusal),
                    edge_manifold_after: mesh_is_edge_manifold(mesh),
                },
                RepairAssessment::Work => {
                    let Some(mesh) = self.target_mesh_mut(target) else {
                        continue;
                    };
                    let report = mesh.repair(passes);
                    SceneRepairOutcome {
                        target,
                        fixes: report.fixes,
                        refusal: report.refusal,
                        edge_manifold_after: report.edge_manifold_after,
                    }
                }
            };
            outcomes.push(outcome);
        }
        outcomes
    }
}

/// The edge-manifold witness-mint attempt, for outcomes built without
/// running the passes.
fn mesh_is_edge_manifold(mesh: &IndexedMesh) -> bool {
    mesh.validated_faces()
        .is_ok_and(|faces| faces.validated_edge_manifold().is_ok())
}

#[cfg(test)]
mod scene_repair_tests {
    use std::sync::Arc;

    use super::*;
    use crate::geometry::compute_normal_from_triangle_vertices;
    use crate::primitives::{IndexedTriangle, Vertex};
    use crate::scene::node::{LeafPayload, NodeId};
    use crate::transform::Transform3D;

    fn tetrahedron() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            .into_iter()
            .map(|indices: [u32; 3]| {
                let tri = [
                    vertices[indices[0] as usize],
                    vertices[indices[1] as usize],
                    vertices[indices[2] as usize],
                ];
                let normal = compute_normal_from_triangle_vertices(&tri).expect("valid triangle");
                IndexedTriangle::from_raw_indices(normal, indices)
            })
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn flipped_tetrahedron() -> IndexedMesh {
        let mut mesh = tetrahedron();
        mesh.faces[0].vertices.reverse();
        mesh
    }

    fn nan_tetrahedron() -> IndexedMesh {
        let mut mesh = tetrahedron();
        mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
        mesh
    }

    /// An open strip of 2 triangles: a hole, with no pass work.
    fn open_strip() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([1.0, 1.0, 0.0]),
        ];
        let faces = [[0, 1, 2], [2, 1, 3]]
            .into_iter()
            .map(|indices: [u32; 3]| {
                let tri = [
                    vertices[indices[0] as usize],
                    vertices[indices[1] as usize],
                    vertices[indices[2] as usize],
                ];
                let normal = compute_normal_from_triangle_vertices(&tri).expect("valid triangle");
                IndexedTriangle::from_raw_indices(normal, indices)
            })
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn add_root_group(graph: &mut Scene) -> NodeId {
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        root
    }

    fn add_inline_leaf(graph: &mut Scene, parent: NodeId, mesh: IndexedMesh) -> NodeId {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(parent, leaf).unwrap();
        leaf
    }

    /// A resource with repairable winding, a refused NaN inline leaf,
    /// and a clean inline leaf: per-target outcomes in walk order, no
    /// global abort, and the refused target does not block the first.
    #[test]
    fn outcomes_are_per_target_in_walk_order_with_no_abort() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let nan_leaf = add_inline_leaf(&mut graph, root, nan_tetrahedron());
        let clean_leaf = add_inline_leaf(&mut graph, root, tetrahedron());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());

        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].target, MeshTarget::Resource(rid));
        assert_eq!(outcomes[0].fixes.len(), 1);
        assert_eq!(outcomes[0].refusal, None);
        assert!(outcomes[0].edge_manifold_after);

        assert_eq!(outcomes[1].target, MeshTarget::InlineLeaf(nan_leaf));
        assert!(outcomes[1].fixes.is_empty());
        assert!(matches!(
            outcomes[1].refusal,
            Some(RepairRefusal::InvalidVertices(_))
        ));

        assert_eq!(outcomes[2].target, MeshTarget::InlineLeaf(clean_leaf));
        assert!(outcomes[2].fixes.is_empty());
        assert_eq!(outcomes[2].refusal, None);
        assert!(outcomes[2].edge_manifold_after);

        // The repaired resource is edge-manifold in the scene, and
        // the refused mesh kept its bytes (NaN compared bitwise).
        assert!(mesh_is_edge_manifold(
            graph.target_mesh(MeshTarget::Resource(rid)).unwrap()
        ));
        let kept = graph.target_mesh(MeshTarget::InlineLeaf(nan_leaf)).unwrap();
        let expected = nan_tetrahedron();
        assert_eq!(kept.faces, expected.faces);
        for (a, b) in kept.vertices.iter().zip(expected.vertices.iter()) {
            assert_eq!(a.coords().map(f32::to_bits), b.coords().map(f32::to_bits));
        }
    }

    /// The S6 economics: with every `Arc` shared to an outside holder
    /// (the GUI worker-clone scenario), only the repaired target
    /// forks; clean and refused targets keep pointer identity, and
    /// the outside holder keeps the pre-repair bytes.
    #[test]
    fn only_repaired_targets_fork_their_shared_storage() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let nan_leaf = add_inline_leaf(&mut graph, root, nan_tetrahedron());
        let clean_leaf = add_inline_leaf(&mut graph, root, tetrahedron());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();

        // Outside holders, as a cloned live document would hold them.
        let held_res = Arc::clone(graph.mesh_resource_arc(rid).unwrap());
        let held_nan = Arc::clone(graph.leaf_mesh_arc(nan_leaf).unwrap());
        let held_clean = Arc::clone(graph.leaf_mesh_arc(clean_leaf).unwrap());

        graph.repair_meshes(RepairPasses::default_repair());

        // Repaired: forked away from the holder, which kept the
        // pre-repair (still flipped) bytes.
        assert!(!Arc::ptr_eq(
            graph.mesh_resource_arc(rid).unwrap(),
            &held_res
        ));
        assert_eq!(*held_res, flipped_tetrahedron());
        // Refused and clean: pointer identity preserved.
        assert!(Arc::ptr_eq(
            graph.leaf_mesh_arc(nan_leaf).unwrap(),
            &held_nan
        ));
        assert!(Arc::ptr_eq(
            graph.leaf_mesh_arc(clean_leaf).unwrap(),
            &held_clean
        ));
    }

    /// A hole is a verdict, not a refusal, and does not fork.
    #[test]
    fn a_holed_target_reports_false_verdict_without_work() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let leaf = add_inline_leaf(&mut graph, root, open_strip());
        let held = Arc::clone(graph.leaf_mesh_arc(leaf).unwrap());

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].fixes.is_empty());
        assert_eq!(outcomes[0].refusal, None);
        assert!(!outcomes[0].edge_manifold_after);
        assert!(Arc::ptr_eq(graph.leaf_mesh_arc(leaf).unwrap(), &held));
    }

    /// Parity with the shared walk and the health sweep: the driver
    /// covers exactly `mesh_targets()`, including a registered
    /// subtree's inline leaf, and every leaf the health sweep flags
    /// resolves to a covered target.
    #[test]
    fn driver_targets_match_the_walk_and_cover_health_flags() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        add_inline_leaf(&mut graph, root, open_strip());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();
        let sub_root = graph.add_group(None, Transform3D::Identity);
        add_inline_leaf(&mut graph, sub_root, open_strip());
        graph.register_subtree_resource(sub_root).unwrap();

        let expected_targets = graph.mesh_targets();
        let flagged = graph.non_edge_manifold_leaves();

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());
        let outcome_targets: Vec<MeshTarget> = outcomes.iter().map(|o| o.target).collect();

        assert_eq!(outcome_targets, expected_targets);
        for leaf in flagged {
            let target = graph.leaf_mesh_target(leaf).unwrap();
            assert!(
                outcome_targets.contains(&target),
                "health-flagged leaf {leaf:?} not covered by the driver"
            );
        }
    }
}
