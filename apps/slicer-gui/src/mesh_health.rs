/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mesh-health status captured at load time. A scene whose meshes are all
//! closed and edge-manifold is safe to edit and slice; anything with detected
//! problems (or not yet checked) opens Transform with its repair card in
//! front.

use sc_scene::Scene;

/// Whether a loaded scene's meshes are known to be sound.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum MeshHealth {
    /// Not checked yet: no scene loaded, or the pre-validation state.
    #[default]
    Unknown,
    /// Every leaf is closed and edge-manifold.
    ProblemFree,
    /// At least 1 leaf's mesh is not closed and edge-manifold.
    HasProblems,
}

/// Classifies a scene by validating every mesh (face validation and the
/// edge-manifold check). A scene with no problematic leaves is `ProblemFree`; 1
/// or more defective leaves make it `HasProblems`. An empty scene has no
/// problematic leaves and so reports `ProblemFree`.
pub(crate) fn scene_mesh_health(scene: &Scene) -> MeshHealth {
    if scene.non_edge_manifold_leaves().is_empty() {
        MeshHealth::ProblemFree
    } else {
        MeshHealth::HasProblems
    }
}

/// Whether a freshly loaded scene of this health should lead the Transform
/// inspector with the repair card: anything but a confirmed problem-free
/// scene.
pub(crate) fn needs_repair(health: MeshHealth) -> bool {
    !matches!(health, MeshHealth::ProblemFree)
}

#[cfg(test)]
mod mesh_health_tests {
    use std::sync::Arc;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_scene::{LeafPayload, NodeId};

    use super::*;

    fn scene_with_root() -> (Scene, NodeId) {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        (graph, root)
    }

    fn add_inline_leaf(graph: &mut Scene, parent: NodeId, mesh: IndexedMesh) {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(parent, leaf).unwrap();
    }

    /// Returns a closed, consistently wound tetrahedron: a valid manifold.
    fn tetrahedron_mesh() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Returns a lone triangle: every edge is a boundary edge, not a manifold.
    fn open_triangle_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

    #[test]
    fn a_closed_manifold_scene_is_problem_free() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        assert_eq!(scene_mesh_health(&graph), MeshHealth::ProblemFree);
    }

    #[test]
    fn a_scene_with_an_open_mesh_has_problems() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, open_triangle_mesh());
        assert_eq!(scene_mesh_health(&graph), MeshHealth::HasProblems);
    }

    #[test]
    fn an_empty_scene_is_problem_free() {
        assert_eq!(scene_mesh_health(&Scene::new()), MeshHealth::ProblemFree);
    }

    #[test]
    fn only_a_problem_free_scene_skips_the_repair_card() {
        assert!(!needs_repair(MeshHealth::ProblemFree));
        assert!(needs_repair(MeshHealth::HasProblems));
        assert!(needs_repair(MeshHealth::Unknown));
    }

    #[test]
    fn default_health_is_unknown() {
        assert_eq!(MeshHealth::default(), MeshHealth::Unknown);
    }
}
