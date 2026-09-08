/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use float_cmp::assert_approx_eq;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

use crate::Scene;
use crate::error::SceneError;
use crate::node::{CsgOp, LeafPayload};

/// Build a closed axis-aligned box, outward-wound, spanning
/// `min`..`max`.
fn box_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
    let corners = [
        [min[0], min[1], min[2]],
        [max[0], min[1], min[2]],
        [max[0], max[1], min[2]],
        [min[0], max[1], min[2]],
        [min[0], min[1], max[2]],
        [max[0], min[1], max[2]],
        [max[0], max[1], max[2]],
        [min[0], max[1], max[2]],
    ];
    let vertices: Vec<Vertex> = corners.into_iter().map(Vertex::new).collect();
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ]
    .into_iter()
    .map(|indices: [u32; 3]| {
        let corner = |i: u32| vertices[i as usize];
        let normal = compute_normal_from_triangle_vertices(&[
            corner(indices[0]),
            corner(indices[1]),
            corner(indices[2]),
        ])
        .expect("a box face is never degenerate");
        IndexedTriangle::from_raw_indices(normal, indices)
    })
    .collect();

    IndexedMesh::new(None, vertices, faces)
}

fn unit_cube() -> IndexedMesh {
    box_mesh([0.0; 3], [1.0; 3])
}

/// Attach one leaf carrying `mesh` under `transform` as a scene root.
fn scene_with_leaf(mesh: IndexedMesh, transform: Transform3D) -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("add leaf");
    scene.add_root(leaf).expect("leaf as root");
    scene
}

fn volume_of(scene: &Scene) -> f64 {
    scene.world_volume_mm3().expect("the fixtures are valid")
}

#[test]
fn an_empty_scene_encloses_nothing() {
    assert_eq!(volume_of(&Scene::new()), 0.0);
}

#[test]
fn a_single_leaf_at_identity_reports_its_own_volume() {
    let scene = scene_with_leaf(unit_cube(), Transform3D::Identity);
    assert_approx_eq!(f64, volume_of(&scene), 1.0);
}

#[test]
fn translation_leaves_the_volume_alone() {
    let scene = scene_with_leaf(
        unit_cube(),
        Transform3D::Affine(AffineTransform3D::from_translation([100.0, -50.0, 7.0])),
    );
    assert_approx_eq!(f64, volume_of(&scene), 1.0, epsilon = 1e-9);
}

#[test]
fn uniform_scale_cubes_the_volume() {
    let scene = scene_with_leaf(
        unit_cube(),
        Transform3D::Affine(AffineTransform3D::from_uniform_scale(3.0)),
    );
    assert_approx_eq!(f64, volume_of(&scene), 27.0, epsilon = 1e-9);
}

#[test]
fn non_uniform_scale_multiplies_the_three_factors() {
    let scene = scene_with_leaf(
        unit_cube(),
        Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 5.0])),
    );
    assert_approx_eq!(f64, volume_of(&scene), 30.0, epsilon = 1e-9);
}

#[test]
fn rotation_leaves_the_volume_alone() {
    let scene = scene_with_leaf(
        unit_cube(),
        Transform3D::Affine(AffineTransform3D::from_rotation_z(0.7)),
    );
    assert_approx_eq!(f64, volume_of(&scene), 1.0, epsilon = 1e-9);
}

#[test]
fn a_mirroring_transform_still_reports_material() {
    // A negative determinant must not subtract volume.
    let scene = scene_with_leaf(
        unit_cube(),
        Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-2.0, 1.0, 1.0])),
    );
    assert_approx_eq!(f64, volume_of(&scene), 2.0, epsilon = 1e-9);
}

#[test]
fn an_inward_wound_mesh_still_reports_material() {
    let mut mesh = unit_cube();
    for face in &mut mesh.faces {
        let [a, b, c] = face.vertices;
        face.vertices = [c, b, a];
    }
    let scene = scene_with_leaf(mesh, Transform3D::Identity);
    assert_approx_eq!(f64, volume_of(&scene), 1.0);
}

#[test]
fn two_leaves_add_rather_than_cancel() {
    // One outward-wound, one inward-wound: a signed sum would give 0.
    let mut inverted = unit_cube();
    for face in &mut inverted.faces {
        let [a, b, c] = face.vertices;
        face.vertices = [c, b, a];
    }
    let mut scene = Scene::new();
    for mesh in [unit_cube(), inverted] {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        scene.add_root(leaf).expect("leaf as root");
    }
    assert_approx_eq!(f64, volume_of(&scene), 2.0);
}

#[test]
fn nested_transforms_compose_before_scaling() {
    // Group scales by 2, leaf scales by 3: the leaf sees 6x on each
    // axis, so 216x the volume, not 8 + 27 or 8 * 3.
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_uniform_scale(3.0)),
            LeafPayload::Mesh(Arc::new(unit_cube())),
        )
        .expect("add leaf");
    let group = scene.add_group(
        None,
        Transform3D::Affine(AffineTransform3D::from_uniform_scale(2.0)),
    );
    scene.add_child(group, leaf).expect("attach leaf");
    scene.add_root(group).expect("group as root");
    assert_approx_eq!(f64, volume_of(&scene), 216.0, epsilon = 1e-9);
}

#[test]
fn a_mesh_resource_shared_by_two_leaves_counts_once_per_leaf() {
    let mut scene = Scene::new();
    let rid = scene.add_mesh_resource(unit_cube());
    for _ in 0..2 {
        let leaf = scene
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .expect("add leaf");
        scene.add_root(leaf).expect("leaf as root");
    }
    assert_approx_eq!(f64, volume_of(&scene), 2.0);
}

#[test]
fn a_referenced_subtree_counts_once_per_occurrence() {
    let mut scene = Scene::new();
    let inner = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube())),
        )
        .expect("add leaf");
    let subtree_root = scene.add_group(None, Transform3D::Identity);
    scene.add_child(subtree_root, inner).expect("attach leaf");
    let rid = scene
        .register_subtree_resource(subtree_root)
        .expect("register subtree");

    for scale in [1.0, 2.0] {
        let reference = scene
            .add_reference(
                None,
                Transform3D::Affine(AffineTransform3D::from_uniform_scale(scale)),
                rid,
            )
            .expect("add reference");
        scene.add_root(reference).expect("reference as root");
    }
    // 1x plus 2x scaled: 1 + 8.
    assert_approx_eq!(f64, volume_of(&scene), 9.0, epsilon = 1e-9);
}

#[test]
fn csg_children_are_summed_conservatively() {
    // A Difference of 2 overlapping cubes reports both, an upper
    // bound, exactly as the AABB query treats CSG.
    let mut scene = Scene::new();
    let big = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube())),
        )
        .expect("add leaf");
    let small = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_uniform_scale(0.5)),
            LeafPayload::Mesh(Arc::new(unit_cube())),
        )
        .expect("add leaf");
    let csg = scene
        .add_csg(
            None,
            Transform3D::Identity,
            CsgOp::Difference,
            vec![big, small],
        )
        .expect("add csg");
    scene.add_root(csg).expect("csg as root");
    assert_approx_eq!(f64, volume_of(&scene), 1.125, epsilon = 1e-9);
}

#[test]
fn a_leaf_whose_faces_do_not_validate_is_reported_with_its_node() {
    let mut mesh = unit_cube();
    mesh.faces[0] = IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 99]);
    let scene = scene_with_leaf(mesh, Transform3D::Identity);
    let root = scene.roots()[0];

    match scene.world_volume_mm3() {
        Err(SceneError::InvalidMeshFaces { node_id, errors }) => {
            assert_eq!(node_id, root);
            assert!(!errors.is_empty());
        }
        other => panic!("expected InvalidMeshFaces, got {other:?}"),
    }
}
