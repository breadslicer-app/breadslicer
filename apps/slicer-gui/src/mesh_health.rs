/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mesh-health status captured at load time and the initial workspace
//! section derived from it. A scene whose meshes are all closed and
//! edge-manifold is safe to edit and slice, so it opens in Transform;
//! anything with detected problems (or not yet checked) opens in
//! Prepare, the home of the future mesh-repair tooling.

use sc_mesh_core::scene::Scene;

use crate::WorkspaceSection;

/// Whether a loaded scene's meshes are known to be sound.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum MeshHealth {
    /// Not checked yet: no scene loaded, or the pre-validation state.
    #[default]
    Unknown,
    /// Every leaf is closed and edge-manifold.
    ProblemFree,
    /// At least one leaf's mesh is not closed and edge-manifold.
    HasProblems,
}

/// Classifies a scene by validating every mesh (faces + edge-manifold). A
/// scene with no problematic leaves is `ProblemFree`; one or more
/// defective leaves make it `HasProblems`. An empty scene has no
/// problematic leaves and so reports `ProblemFree`.
pub(crate) fn scene_mesh_health(scene: &Scene) -> MeshHealth {
    if scene.non_edge_manifold_leaves().is_empty() {
        MeshHealth::ProblemFree
    } else {
        MeshHealth::HasProblems
    }
}

/// The workspace section a freshly loaded scene should open in, given
/// its mesh health. Only a confirmed problem-free scene opens straight
/// into Transform; problems or an unknown status land in Prepare.
pub(crate) fn initial_section(health: MeshHealth) -> WorkspaceSection {
    match health {
        MeshHealth::ProblemFree => WorkspaceSection::Transform,
        MeshHealth::Unknown | MeshHealth::HasProblems => WorkspaceSection::Prepare,
    }
}

#[cfg(test)]
mod mesh_health_tests {
    use std::sync::Arc;

    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, LeafPayload, NodeId};

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

    /// A closed, consistently wound tetrahedron: a valid manifold.
    fn tetrahedron_mesh() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            .into_iter()
            .map(|indices| IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), indices))
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// A lone triangle: every edge is a boundary edge, not a manifold.
    fn open_triangle_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
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
    fn problem_free_opens_in_transform() {
        assert!(matches!(
            initial_section(MeshHealth::ProblemFree),
            WorkspaceSection::Transform
        ));
    }

    #[test]
    fn problems_open_in_prepare() {
        assert!(matches!(
            initial_section(MeshHealth::HasProblems),
            WorkspaceSection::Prepare
        ));
    }

    #[test]
    fn unknown_opens_in_prepare() {
        assert!(matches!(
            initial_section(MeshHealth::Unknown),
            WorkspaceSection::Prepare
        ));
    }

    #[test]
    fn default_health_is_unknown() {
        assert_eq!(MeshHealth::default(), MeshHealth::Unknown);
    }
}
