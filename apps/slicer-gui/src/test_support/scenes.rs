/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};

/// Returns a mesh of 1 vertex and no face.
pub(crate) fn empty_leaf_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
        faces: vec![],
    }
}

/// Builds the graph that a loader makes for 1 object, with an
/// [`empty_leaf_mesh`] leaf under an identity-Group root.
pub(crate) fn graph_with_one_leaf() -> Scene {
    graph_with_one_leaf_of(empty_leaf_mesh())
}

/// Builds the graph that a loader makes for 1 object, with a leaf of no vertex
/// under an identity-Group root.
pub(crate) fn graph_with_one_object() -> Scene {
    graph_with_one_leaf_of(IndexedMesh::new(None, Vec::new(), Vec::new()))
}

fn graph_with_one_leaf_of(mesh: IndexedMesh) -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("add leaf");
    g.add_child(root, leaf).expect("attach leaf");
    g
}

/// Builds a scene with 1 cube leaf as its own root, spanning z = -0.5 to 0.5,
/// so half of it stands above the plate.
pub(crate) fn cube_scene() -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    scene
}
