/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mesh-health verdicts captured at load time: whether each stored mesh
//! passes the slicer's gate. The load worker checks every mesh so the UI
//! thread receives ready verdicts; the Transform tab's MESH CHECK card
//! (`mesh_check`) remembers them and leads the tab with the repair card when
//! a mesh does not slice.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_scene::Scene;

use crate::mesh_check::{mesh_slices, target_arc};

/// Each stored mesh of a scene, with whether it slices.
pub(crate) type MeshVerdicts = Vec<(Arc<IndexedMesh>, bool)>;

/// Checks every stored mesh of `scene` (vertices, faces and the edge check).
/// A mesh without faces counts as sound: it is the support set of a project,
/// kept as data.
pub(crate) fn scene_mesh_verdicts(scene: &Scene) -> MeshVerdicts {
    scene
        .mesh_targets()
        .into_iter()
        .filter_map(|target| target_arc(scene, target))
        .map(|mesh| {
            let sliceable = mesh_slices(&mesh);
            (mesh, sliceable)
        })
        .collect()
}

/// Whether every mesh of `scene` slices.
#[cfg(test)]
pub(crate) fn scene_slices(scene: &Scene) -> bool {
    scene_mesh_verdicts(scene).iter().all(|(_, ok)| *ok)
}

#[cfg(test)]
mod mesh_health_tests {
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
    fn a_closed_manifold_scene_slices() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        assert!(scene_slices(&graph));
    }

    #[test]
    fn a_scene_with_an_open_mesh_does_not_slice() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        add_inline_leaf(&mut graph, root, open_triangle_mesh());
        let verdicts = scene_mesh_verdicts(&graph);
        assert_eq!(
            verdicts.iter().map(|(_, ok)| *ok).collect::<Vec<_>>(),
            [true, false]
        );
        assert!(!scene_slices(&graph));
    }

    #[test]
    fn a_mesh_without_faces_is_no_problem() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(
            &mut graph,
            root,
            IndexedMesh {
                meta: None,
                vertices: Vec::new(),
                faces: Vec::new(),
            },
        );
        assert!(scene_slices(&graph));
    }

    #[test]
    fn an_empty_scene_slices() {
        assert!(scene_slices(&Scene::new()));
    }

    #[test]
    fn a_mesh_with_a_nan_vertex_does_not_slice() {
        let (mut graph, root) = scene_with_root();
        let mut mesh = tetrahedron_mesh();
        mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
        add_inline_leaf(&mut graph, root, mesh);
        assert!(!scene_slices(&graph));
    }
}
