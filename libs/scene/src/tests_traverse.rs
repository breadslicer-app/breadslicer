/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the structural traversal and world-transform ports.

use std::sync::Arc;

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

use crate::Scene;
use crate::error::SceneError;
use crate::expand::CsgPolicy;
use crate::node::{CsgOp, LeafPayload, NodeId};
use crate::traverse::ReferencePolicy;

fn triangle_arc() -> Arc<IndexedMesh> {
    Arc::new(IndexedMesh {
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
    })
}

fn translation(t: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation(t))
}

fn mesh_leaf(g: &mut Scene, local: Transform3D) -> NodeId {
    g.add_leaf(None, local, LeafPayload::Mesh(triangle_arc()))
        .expect("fresh leaf installs")
}

/// Two scene roots plus a registered subtree referenced twice:
///
/// - `g_outer` (Group): `leaf_a`, `g_inner` (Group: `leaf_b`,
///   a tombstoned leaf, `csg` (Union: `op1`, `op2`)), `ref1`, `ref2`
/// - `leaf_r` (root leaf)
/// - registered subtree: `sub_root` (Group) holding `sub_leaf`
struct Fixture {
    g: Scene,
    g_outer: NodeId,
    g_inner: NodeId,
    leaf_a: NodeId,
    leaf_b: NodeId,
    csg: NodeId,
    op1: NodeId,
    op2: NodeId,
    ref1: NodeId,
    ref2: NodeId,
    leaf_r: NodeId,
    sub_root: NodeId,
    sub_leaf: NodeId,
}

fn fixture() -> Fixture {
    let mut g = Scene::new();
    let g_outer = g.add_group(None, translation([1.0, 0.0, 0.0]));
    g.add_root(g_outer).expect("outer group roots");
    let leaf_a = mesh_leaf(&mut g, Transform3D::Identity);
    g.add_child(g_outer, leaf_a).expect("attach leaf_a");
    let g_inner = g.add_group(None, translation([0.0, 2.0, 0.0]));
    g.add_child(g_outer, g_inner).expect("attach inner group");
    let leaf_b = mesh_leaf(&mut g, translation([0.0, 0.0, 3.0]));
    g.add_child(g_inner, leaf_b).expect("attach leaf_b");
    let tomb = mesh_leaf(&mut g, Transform3D::Identity);
    g.add_child(g_inner, tomb).expect("attach doomed leaf");
    let op1 = mesh_leaf(&mut g, Transform3D::Identity);
    let op2 = mesh_leaf(&mut g, Transform3D::Identity);
    let csg = g
        .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![op1, op2])
        .expect("csg installs");
    g.add_child(g_inner, csg).expect("attach csg");
    g.remove_leaf(tomb).expect("tombstone the doomed leaf");
    let sub_leaf = mesh_leaf(&mut g, translation([0.0, 0.0, 7.0]));
    let sub_root = g.add_group(None, translation([5.0, 0.0, 0.0]));
    g.add_child(sub_root, sub_leaf)
        .expect("attach subtree leaf");
    let rid = g
        .register_subtree_resource(sub_root)
        .expect("unparented group registers");
    let ref1 = g
        .add_reference(None, Transform3D::Identity, rid)
        .expect("first reference installs");
    g.add_child(g_outer, ref1).expect("attach ref1");
    let ref2 = g
        .add_reference(None, translation([9.0, 0.0, 0.0]), rid)
        .expect("second reference installs");
    g.add_child(g_outer, ref2).expect("attach ref2");
    let leaf_r = mesh_leaf(&mut g, translation([0.0, 4.0, 0.0]));
    g.add_root(leaf_r).expect("leaf roots");
    Fixture {
        g,
        g_outer,
        g_inner,
        leaf_a,
        leaf_b,
        csg,
        op1,
        op2,
        ref1,
        ref2,
        leaf_r,
        sub_root,
        sub_leaf,
    }
}

#[test]
fn reachable_leaves_descend_skip_is_pre_order_without_refs_or_tombstones() {
    let f = fixture();
    let leaves =
        f.g.reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
            .expect("no refusal under (Descend, Skip)");
    assert_eq!(leaves, vec![f.leaf_a, f.leaf_b, f.op1, f.op2, f.leaf_r]);
}

#[test]
fn refuse_csg_names_the_csg_node() {
    let f = fixture();
    let result =
        f.g.reachable_leaves(CsgPolicy::Refuse, ReferencePolicy::Skip);
    assert_eq!(result, Err(SceneError::UnevaluatedCsg { node_id: f.csg }));
}

#[test]
fn refuse_reference_names_the_first_reference_in_pre_order() {
    let f = fixture();
    let result =
        f.g.reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Refuse);
    assert_eq!(
        result,
        Err(SceneError::UnexpandedReference { node_id: f.ref1 })
    );
}

#[test]
fn all_policies_agree_on_a_csg_free_reference_free_scene() {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group roots");
    let first = mesh_leaf(&mut g, Transform3D::Identity);
    g.add_child(root, first).expect("attach first");
    let second = mesh_leaf(&mut g, Transform3D::Identity);
    g.add_child(root, second).expect("attach second");
    let expected = vec![first, second];
    for csg in [CsgPolicy::Descend, CsgPolicy::Refuse] {
        for refs in [ReferencePolicy::Skip, ReferencePolicy::Refuse] {
            assert_eq!(
                g.reachable_leaves(csg, refs).expect("nothing to refuse"),
                expected
            );
        }
    }
}

#[test]
fn for_each_reachable_node_visits_every_live_structural_node_in_pre_order() {
    let f = fixture();
    let mut visited = Vec::new();
    f.g.for_each_reachable_node(|id, _node| visited.push(id));
    assert_eq!(
        visited,
        vec![
            f.g_outer, f.leaf_a, f.g_inner, f.leaf_b, f.csg, f.op1, f.op2, f.ref1, f.ref2, f.leaf_r
        ]
    );
    assert!(!visited.contains(&f.sub_root));
    assert!(!visited.contains(&f.sub_leaf));
}

#[test]
fn world_transform_of_composes_the_ancestor_chain_root_applied_last() {
    let f = fixture();
    let expected = Transform3D::Identity
        .compose(&translation([1.0, 0.0, 0.0]))
        .compose(&translation([0.0, 2.0, 0.0]))
        .compose(&translation([0.0, 0.0, 3.0]));
    assert_eq!(f.g.world_transform_of(f.leaf_b), Ok(expected));
}

#[test]
fn world_transform_of_a_root_leaf_is_its_local_transform() {
    let f = fixture();
    let expected = Transform3D::Identity.compose(&translation([0.0, 4.0, 0.0]));
    assert_eq!(f.g.world_transform_of(f.leaf_r), Ok(expected));
}

#[test]
fn world_transform_of_stops_at_the_subtree_root() {
    let f = fixture();
    let expected = Transform3D::Identity
        .compose(&translation([5.0, 0.0, 0.0]))
        .compose(&translation([0.0, 0.0, 7.0]));
    assert_eq!(f.g.world_transform_of(f.sub_leaf), Ok(expected));
}

#[test]
fn world_transform_of_an_unbound_id_is_a_dangling_error() {
    let mut g = Scene::new();
    let unbound = g.allocate_unbound_node_id();
    assert_eq!(
        g.world_transform_of(unbound),
        Err(SceneError::DanglingNodeId { id: unbound })
    );
}

#[test]
fn world_transform_of_agrees_with_the_expansion_walk_on_attached_leaves() {
    let f = fixture();
    let mut checked = 0usize;
    f.g.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Descend, |id, world, _mesh| {
        // Occurrences under a Reference carry call-site transforms
        // the structural chain cannot know; compare attached leaves
        // only (the fixture's referenced subtree hangs off ref1/ref2).
        if id != f.sub_leaf {
            assert_eq!(f.g.world_transform_of(id), Ok(*world));
            checked += 1;
        }
        Ok(())
    })
    .expect("fixture expands");
    assert_eq!(checked, 5);
}

#[test]
fn a_closure_error_stops_the_leaf_walk() {
    let f = fixture();
    let mut visited = Vec::new();
    let result = f.g.for_each_reachable_leaf::<SceneError, _>(
        CsgPolicy::Descend,
        ReferencePolicy::Skip,
        |id| {
            visited.push(id);
            if visited.len() == 2 {
                Err(SceneError::DanglingNodeId { id })
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(visited, vec![f.leaf_a, f.leaf_b]);
}

#[test]
fn reachable_leaf_count_matches_the_walk() {
    let f = fixture();
    let leaves =
        f.g.reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
            .expect("no refusal under (Descend, Skip)");
    assert_eq!(f.g.reachable_leaf_count(), leaves.len());
    assert_eq!(f.g.first_leaf_id(), leaves.first().copied());
}
