/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Capability pins for the OBJ writer over scenes with subtree references:
//! every occurrence of an referenced subtree serialises in full, in
//! scene pre-order, under the occurrence's composed transform, with
//! mesh names taken verbatim (duplicate `o` markers are the standing
//! representation for repeated names). These pins are permanent:
//! a refusal, witness, or reference-skipping walk on this path must
//! turn them red, never ship green.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{
    CsgPolicy, IndexedMesh, IndexedTriangle, LeafPayload, MAX_EXPANDED_LEAF_COUNT, MeshMetadata,
    NodeId, Normal, Scene, SubtreeResourceId, Vertex,
};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};

use crate::error::WriteObjError;

use super::write_obj;

fn triangle(name: Option<&str>) -> IndexedMesh {
    IndexedMesh {
        meta: name.map(|n| MeshMetadata {
            name: Some(n.to_string()),
            ..Default::default()
        }),
        vertices: vec![
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.577, 0.577, 0.577]),
            [0, 1, 2],
        )],
    }
}

/// Registers a single-triangle subtree resource (identity group over
/// one leaf) and returns its id.
fn register_triangle_subtree(scene: &mut Scene, name: Option<&str>) -> SubtreeResourceId {
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(triangle(name))),
        )
        .expect("add subtree leaf");
    let root = scene.add_group(None, Transform3D::Identity);
    scene.add_child(root, leaf).expect("attach subtree leaf");
    scene
        .register_subtree_resource(root)
        .expect("unparented group registers")
}

fn add_reference_at(scene: &mut Scene, parent: NodeId, rid: SubtreeResourceId, t: [f64; 3]) {
    let reference = scene
        .add_reference(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(t)),
            rid,
        )
        .expect("registered resource resolves");
    scene
        .add_child(parent, reference)
        .expect("attach reference");
}

fn write_to_string(scene: &Scene) -> String {
    let mut buf: Vec<u8> = Vec::new();
    write_obj(scene, &mut buf).expect("write_obj succeeds");
    String::from_utf8(buf).expect("output is valid UTF-8")
}

#[test]
fn a_single_occurrence_serialises_like_a_plain_leaf() {
    let mut g = Scene::new();
    let rid = register_triangle_subtree(&mut g, None);
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    g.add_root(root).expect("add root");
    assert_eq!(write_to_string(&g), "v 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
}

#[test]
fn two_references_serialise_two_occurrences_with_verbatim_names() {
    let mut g = Scene::new();
    let rid = register_triangle_subtree(&mut g, Some("Part"));
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    add_reference_at(&mut g, root, rid, [10.0, 0.0, 0.0]);
    g.add_root(root).expect("add root");
    assert_eq!(
        write_to_string(&g),
        "o Part\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n\
         o Part\nv 11 0 0\nv 10 1 0\nv 10 0 1\nf 4 5 6\n"
    );
}

#[test]
fn nested_references_serialise_the_full_product_of_occurrences() {
    // Inner: one triangle. Outer: 2 inner references at x = +2 / +4.
    // Scene: 2 outer references at y = 0 / +10. Expansion: 2 x 2 = 4
    // occurrences, pre-order, each under its composed translation.
    let mut g = Scene::new();
    let inner = register_triangle_subtree(&mut g, None);
    let outer_root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, outer_root, inner, [2.0, 0.0, 0.0]);
    add_reference_at(&mut g, outer_root, inner, [4.0, 0.0, 0.0]);
    let outer = g
        .register_subtree_resource(outer_root)
        .expect("unparented group registers");
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, outer, [0.0, 0.0, 0.0]);
    add_reference_at(&mut g, root, outer, [0.0, 10.0, 0.0]);
    g.add_root(root).expect("add root");
    assert_eq!(
        write_to_string(&g),
        "o\nv 3 0 0\nv 2 1 0\nv 2 0 1\nf 1 2 3\n\
         o\nv 5 0 0\nv 4 1 0\nv 4 0 1\nf 4 5 6\n\
         o\nv 3 10 0\nv 2 11 0\nv 2 10 1\nf 7 8 9\n\
         o\nv 5 10 0\nv 4 11 0\nv 4 10 1\nf 10 11 12\n"
    );
}

#[test]
fn occurrences_keep_their_position_in_scene_order() {
    let mut g = Scene::new();
    let rid = register_triangle_subtree(&mut g, Some("S"));
    let root = g.add_group(None, Transform3D::Identity);
    let a = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(triangle(Some("A")))),
        )
        .expect("add leaf A");
    g.add_child(root, a).expect("attach leaf A");
    add_reference_at(&mut g, root, rid, [10.0, 0.0, 0.0]);
    let b = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([20.0, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(triangle(Some("B")))),
        )
        .expect("add leaf B");
    g.add_child(root, b).expect("attach leaf B");
    g.add_root(root).expect("add root");
    assert_eq!(
        write_to_string(&g),
        "o A\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n\
         o S\nv 11 0 0\nv 10 1 0\nv 10 0 1\nf 4 5 6\n\
         o B\nv 21 0 0\nv 20 1 0\nv 20 0 1\nf 7 8 9\n"
    );
}

#[test]
fn a_referenced_prism_trio_serialises_every_occurrence_in_full() {
    let meshes = [
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ];
    let vertex_lines: usize = meshes.iter().map(|m| m.vertices().len()).sum();
    let face_lines: usize = meshes.iter().map(|m| m.faces().len()).sum();

    let mut g = Scene::new();
    let sub_root = g.add_group(None, Transform3D::Identity);
    for (i, mesh) in meshes.iter().enumerate() {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([
                    i as f64 * 30.0,
                    0.0,
                    0.0,
                ])),
                LeafPayload::Mesh(Arc::new(mesh.clone())),
            )
            .expect("add trio leaf");
        g.add_child(sub_root, leaf).expect("attach trio leaf");
    }
    let rid = g
        .register_subtree_resource(sub_root)
        .expect("unparented group registers");
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    add_reference_at(&mut g, root, rid, [0.0, 100.0, 0.0]);
    g.add_root(root).expect("add root");

    let out = write_to_string(&g);
    assert_eq!(
        out.lines().filter(|l| l.starts_with("v ")).count(),
        2 * vertex_lines
    );
    assert_eq!(
        out.lines().filter(|l| l.starts_with("f ")).count(),
        2 * face_lines
    );

    // Both occurrences of the cube's first vertex are present: the
    // first verbatim, the second shifted 100mm along +Y.
    let c = unit_cube_mesh().vertices()[0];
    let shifted = AffineTransform3D::from_translation([0.0, 100.0, 0.0]).apply_to_vertex(c);
    let first = format!("v {} {} {}", c[0], c[1], c[2]);
    let second = format!("v {} {} {}", shifted[0], shifted[1], shifted[2]);
    assert!(out.contains(&first), "first cube occurrence missing");
    assert!(out.contains(&second), "second cube occurrence missing");
}

/// Registers a chain of subtree resources whose top resource expands
/// to 10^levels triangle occurrences, using O(levels) nodes: each
/// level is a group of 10 references to the previous one.
fn register_power_of_ten_chain(g: &mut Scene, levels: u32) -> SubtreeResourceId {
    let mut rid = register_triangle_subtree(g, None);
    for _ in 0..levels {
        let group = g.add_group(None, Transform3D::Identity);
        for _ in 0..10 {
            let reference = g
                .add_reference(None, Transform3D::Identity, rid)
                .expect("registered resource resolves");
            g.add_child(group, reference)
                .expect("attach chain reference");
        }
        rid = g
            .register_subtree_resource(group)
            .expect("unparented group registers");
    }
    rid
}

#[test]
fn at_cap_expansion_serialises() {
    // The budget is inclusive: a scene expanding to exactly
    // MAX_EXPANDED_LEAF_COUNT occurrences serialises. The count
    // assertion also pins the cap's value through this path, so a
    // lowered cap cannot ship unnoticed.
    let mut g = Scene::new();
    let rid = register_power_of_ten_chain(&mut g, 5);
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    g.add_root(root).expect("add root");
    assert_eq!(
        g.expanded_leaf_count(CsgPolicy::Descend)
            .expect("chain is acyclic"),
        MAX_EXPANDED_LEAF_COUNT
    );
    write_obj(&g, &mut std::io::sink()).expect("at-cap scene serialises");
}

#[test]
fn over_cap_expansion_is_refused_before_any_output() {
    let mut g = Scene::new();
    let rid = register_power_of_ten_chain(&mut g, 5);
    let root = g.add_group(None, Transform3D::Identity);
    add_reference_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    // One plain leaf pushes the expansion to the cap plus 1.
    let extra = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(triangle(None))),
        )
        .expect("add extra leaf");
    g.add_child(root, extra).expect("attach extra leaf");
    g.add_root(root).expect("add root");

    let mut buf: Vec<u8> = Vec::new();
    match write_obj(&g, &mut buf) {
        Err(WriteObjError::LeafBudgetExceeded { limit }) => {
            assert_eq!(limit, MAX_EXPANDED_LEAF_COUNT);
        }
        Err(other) => panic!("wrong error: {other:?}"),
        Ok(()) => panic!("over-cap scene serialised instead of being refused"),
    }
    assert!(buf.is_empty(), "refusal must precede any output bytes");
}

#[test]
fn csg_operands_serialise_as_plain_objects() {
    // The OBJ writer's standing CSG stance: descend into the
    // operands and serialise them as plain objects.
    use sc_mesh_core::CsgOp;

    let mut g = Scene::new();
    let a = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(triangle(Some("A")))),
        )
        .expect("add operand A");
    let b = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(triangle(Some("B")))),
        )
        .expect("add operand B");
    let csg = g
        .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![a, b])
        .expect("add csg");
    g.add_root(csg).expect("add root");
    assert_eq!(
        write_to_string(&g),
        "o A\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n\
         o B\nv 11 0 0\nv 10 1 0\nv 10 0 1\nf 4 5 6\n"
    );
}
