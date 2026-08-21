/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide diagnosis driver: runs the per-mesh diagnosis policy
//! over every stored mesh (the [`Scene::mesh_targets`] walk) and
//! reports one finding set per target. Purely read-side: the shared
//! borrow guarantees no stored `Arc` is ever forked.

use crate::Scene;
use crate::mesh_targets::MeshTarget;
use sc_mesh_core::indexed_mesh::MeshDiagnosis;

impl Scene {
    /// Diagnose every stored mesh and report one
    /// [`MeshDiagnosis`] per target, in [`Scene::mesh_targets`]
    /// order.
    pub fn diagnose_meshes(&self) -> Vec<(MeshTarget, MeshDiagnosis)> {
        self.mesh_targets()
            .into_iter()
            .filter_map(|target| {
                let mesh = self.target_mesh(target)?;
                Some((target, mesh.diagnose()))
            })
            .collect()
    }
}

#[cfg(test)]
mod scene_diagnose_tests {
    use std::sync::Arc;

    use super::*;
    use crate::node::LeafPayload;
    use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
    use sc_mesh_core::indexed_mesh::{FaceDiagnosis, IndexedMesh};
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

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

    /// 2 consistently wound triangles sharing one edge: a hole.
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

    fn diagnosis_says_not_edge_manifold(diagnosis: &MeshDiagnosis) -> bool {
        match &diagnosis.faces {
            FaceDiagnosis::Invalid(_) => true,
            FaceDiagnosis::Valid {
                edge_manifold_errors,
                ..
            } => !edge_manifold_errors.is_empty(),
        }
    }

    /// A clean shared resource, a defective reachable inline leaf,
    /// and a defective inline leaf inside a registered subtree.
    fn combined_scene() -> Scene {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let rid = graph.add_mesh_resource(tetrahedron());
        for _ in 0..2 {
            let leaf = graph
                .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
                .unwrap();
            graph.add_child(root, leaf).unwrap();
        }
        let inline = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(open_strip())),
            )
            .unwrap();
        graph.add_child(root, inline).unwrap();
        let sub_root = graph.add_group(None, Transform3D::Identity);
        let sub_leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(open_strip())),
            )
            .unwrap();
        graph.add_child(sub_root, sub_leaf).unwrap();
        graph.register_subtree_resource(sub_root).unwrap();
        graph
    }

    #[test]
    fn diagnoses_every_target_in_walk_order() {
        let graph = combined_scene();
        let diagnosed: Vec<MeshTarget> = graph
            .diagnose_meshes()
            .into_iter()
            .map(|(target, _)| target)
            .collect();
        assert_eq!(diagnosed, graph.mesh_targets());
    }

    #[test]
    fn defective_targets_report_while_clean_targets_stay_clean() {
        let graph = combined_scene();
        for (target, diagnosis) in graph.diagnose_meshes() {
            match target {
                MeshTarget::Resource(_) => {
                    assert!(!diagnosis.has_problems(), "clean resource flagged")
                }
                MeshTarget::InlineLeaf(_) => {
                    assert!(diagnosis.has_problems(), "holed inline leaf not flagged")
                }
            }
        }
    }

    #[test]
    fn health_and_diagnosis_agree_on_every_leaf() {
        let graph = combined_scene();
        let flagged = graph.non_edge_manifold_leaves();
        let verdicts: std::collections::HashMap<MeshTarget, bool> = graph
            .diagnose_meshes()
            .into_iter()
            .map(|(target, diagnosis)| (target, diagnosis_says_not_edge_manifold(&diagnosis)))
            .collect();
        for (target, says_bad) in &verdicts {
            for leaf in graph.leaves_referencing(*target) {
                assert_eq!(
                    flagged.contains(&leaf),
                    *says_bad,
                    "health and diagnosis disagree on leaf {leaf:?}"
                );
            }
        }
    }
}
