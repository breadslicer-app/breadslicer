/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::NodeKind;

use super::{leaves_as_roots, meshes_as_roots, trio_as_roots};

/// Returns the mesh and the transform of each root, in order, after checking
/// that each root is a leaf without metadata.
fn root_leaves(scene: &Scene) -> Vec<(IndexedMesh, Transform3D)> {
    scene
        .roots()
        .iter()
        .map(|&root| {
            let node = scene.node(root).expect("a root is a node of its scene");
            assert_eq!(node.meta, None);
            assert!(matches!(node.kind, NodeKind::Leaf(_)), "{root:?}");
            let mesh = scene.leaf_mesh_arc(root).expect("a leaf holds a mesh");
            (IndexedMesh::clone(mesh), node.local_transform)
        })
        .collect()
}

#[test]
fn no_mesh_gives_a_scene_without_roots() {
    assert!(meshes_as_roots([]).roots().is_empty());
}

#[test]
fn each_leaf_becomes_a_root_in_order_with_its_transform() {
    let lifted = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0]));
    let scene = leaves_as_roots([
        (unit_cube_mesh(), lifted),
        (cuboid_with_y_tunnel(), Transform3D::Identity),
    ]);
    assert_eq!(
        root_leaves(&scene),
        [
            (unit_cube_mesh(), lifted),
            (cuboid_with_y_tunnel(), Transform3D::Identity),
        ]
    );
}

#[test]
fn each_mesh_becomes_a_root_leaf_in_order_with_an_identity_transform() {
    let scene = meshes_as_roots([cuboid_with_z_tunnel(), unit_cube_mesh()]);
    assert_eq!(
        root_leaves(&scene),
        [
            (cuboid_with_z_tunnel(), Transform3D::Identity),
            (unit_cube_mesh(), Transform3D::Identity),
        ]
    );
}

#[test]
fn the_trio_holds_the_cube_and_the_2_tunnel_prisms_in_order() {
    let scene = trio_as_roots();
    assert_eq!(
        root_leaves(&scene),
        [
            (unit_cube_mesh(), Transform3D::Identity),
            (cuboid_with_z_tunnel(), Transform3D::Identity),
            (cuboid_with_y_tunnel(), Transform3D::Identity),
        ]
    );
    let world = scene
        .aabb()
        .expect("a fresh scene has no dangling id")
        .expect("the trio has geometry");
    assert_eq!(world.min().coords(), [-1.0, -1.0, -0.5]);
    assert_eq!(world.max().coords(), [1.0, 1.0, 2.0]);
}
