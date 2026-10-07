/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::{NodeId, NodeKind};

use super::leaves_under_group_root;

/// Returns the children of `group`, after checking that it is a group with an
/// identity transform and no metadata.
fn group_children(scene: &Scene, group: NodeId) -> &[NodeId] {
    let node = scene.node(group).expect("a group is a node of its scene");
    assert_eq!(node.local_transform, Transform3D::Identity);
    assert_eq!(node.meta, None);
    let NodeKind::Group { children } = &node.kind else {
        panic!("{group:?} is not a group: {:?}", node.kind);
    };
    children
}

/// Returns the mesh and the transform of `leaf`, after checking that it is a
/// leaf without metadata.
fn leaf_content(scene: &Scene, leaf: NodeId) -> (IndexedMesh, Transform3D) {
    let node = scene.node(leaf).expect("a leaf is a node of its scene");
    assert_eq!(node.meta, None);
    assert!(matches!(node.kind, NodeKind::Leaf(_)), "{leaf:?}");
    let mesh = scene.leaf_mesh_arc(leaf).expect("a leaf holds a mesh");
    (IndexedMesh::clone(mesh), node.local_transform)
}

#[test]
fn each_leaf_becomes_a_child_of_the_group_root_in_order_with_its_transform() {
    let lifted = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0]));
    let scene = leaves_under_group_root([
        (unit_cube_mesh(), lifted),
        (cuboid_with_z_tunnel(), Transform3D::Identity),
    ]);
    let &[root] = scene.roots() else {
        panic!("1 root expected: {:?}", scene.roots());
    };
    let leaves: Vec<_> = group_children(&scene, root)
        .iter()
        .map(|&leaf| leaf_content(&scene, leaf))
        .collect();
    assert_eq!(
        leaves,
        [
            (unit_cube_mesh(), lifted),
            (cuboid_with_z_tunnel(), Transform3D::Identity),
        ]
    );
}
