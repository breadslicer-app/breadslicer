/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, MeshMetadata};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::NodeKind;

use super::{leaves_under_group_root, named_trio_under_group_root};

/// Returns the mesh and the transform of each child of the scene's root, in
/// order, after checking that the root is the only one and is a group with an
/// identity transform, and that each child is a leaf; no node has metadata.
fn group_root_leaves(scene: &Scene) -> Vec<(IndexedMesh, Transform3D)> {
    let &[root] = scene.roots() else {
        panic!("1 root expected: {:?}", scene.roots());
    };
    let node = scene.node(root).expect("a root is a node of its scene");
    assert_eq!(node.local_transform, Transform3D::Identity);
    assert_eq!(node.meta, None);
    let NodeKind::Group { children } = &node.kind else {
        panic!("the root is not a group: {:?}", node.kind);
    };
    children
        .iter()
        .map(|&leaf| {
            let node = scene.node(leaf).expect("a child is a node of its scene");
            assert_eq!(node.meta, None);
            assert!(matches!(node.kind, NodeKind::Leaf(_)), "{leaf:?}");
            let mesh = scene.leaf_mesh_arc(leaf).expect("a leaf holds a mesh");
            (IndexedMesh::clone(mesh), node.local_transform)
        })
        .collect()
}

fn translation(offset: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation(offset))
}

#[test]
fn each_leaf_becomes_a_child_of_the_group_root_in_order_with_its_transform() {
    let lifted = translation([0.0, 0.0, 5.0]);
    let scene = leaves_under_group_root([
        (unit_cube_mesh(), lifted),
        (cuboid_with_z_tunnel(), Transform3D::Identity),
    ]);
    assert_eq!(
        group_root_leaves(&scene),
        [
            (unit_cube_mesh(), lifted),
            (cuboid_with_z_tunnel(), Transform3D::Identity),
        ]
    );
}

#[test]
fn the_named_trio_holds_the_3_named_meshes_in_order_at_their_places() {
    let scene = named_trio_under_group_root([0.0; 3]);
    assert_eq!(
        group_root_leaves(&scene),
        [
            (
                IndexedMesh {
                    meta: Some(MeshMetadata {
                        name: Some("Cube".to_string()),
                        header_comment: None,
                    }),
                    ..unit_cube_mesh()
                },
                Transform3D::Identity,
            ),
            (
                IndexedMesh {
                    meta: Some(MeshMetadata {
                        name: Some("ZTunnel".to_string()),
                        header_comment: None,
                    }),
                    ..cuboid_with_z_tunnel()
                },
                translation([5.0, 0.0, 0.0]),
            ),
            (
                IndexedMesh {
                    meta: Some(MeshMetadata {
                        name: Some("YTunnel".to_string()),
                        header_comment: None,
                    }),
                    ..cuboid_with_y_tunnel()
                },
                translation([-5.0, 0.0, 0.0]),
            ),
        ]
    );
}
