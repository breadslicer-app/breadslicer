/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::NodeKind;

use super::{meshes_as_roots, trio_as_roots};

/// Returns the mesh of each root, in order, after checking that each root is a
/// leaf with an identity transform and no metadata.
fn root_meshes(scene: &Scene) -> Vec<IndexedMesh> {
    scene
        .roots()
        .iter()
        .map(|&root| {
            let node = scene.node(root).expect("a root is a node of its scene");
            assert_eq!(node.local_transform, Transform3D::Identity);
            assert_eq!(node.meta, None);
            assert!(matches!(node.kind, NodeKind::Leaf(_)), "{root:?}");
            let mesh = scene.leaf_mesh_arc(root).expect("a leaf holds a mesh");
            IndexedMesh::clone(mesh)
        })
        .collect()
}

#[test]
fn no_mesh_gives_a_scene_without_roots() {
    assert!(meshes_as_roots([]).roots().is_empty());
}

#[test]
fn each_mesh_becomes_a_root_leaf_in_order_with_an_identity_transform() {
    let scene = meshes_as_roots([cuboid_with_z_tunnel(), unit_cube_mesh()]);
    assert_eq!(
        root_meshes(&scene),
        [cuboid_with_z_tunnel(), unit_cube_mesh()]
    );
}

#[test]
fn the_trio_holds_the_cube_and_the_2_tunnel_prisms_in_order() {
    let scene = trio_as_roots();
    assert_eq!(
        root_meshes(&scene),
        [
            unit_cube_mesh(),
            cuboid_with_z_tunnel(),
            cuboid_with_y_tunnel(),
        ]
    );
    let world = scene
        .aabb()
        .expect("a fresh scene has no dangling id")
        .expect("the trio has geometry");
    assert_eq!(world.min().coords(), [-1.0, -1.0, -0.5]);
    assert_eq!(world.max().coords(), [1.0, 1.0, 2.0]);
}
