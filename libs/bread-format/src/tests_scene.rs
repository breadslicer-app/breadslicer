/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{CsgOp, LeafPayload, NodeKind, Scene, SceneMetadata};

use crate::scene::{DecodeSceneError, decode_scene, encode_scene};
use crate::tests_mesh::grid;

fn named(name: &str) -> Option<SceneMetadata> {
    Some(SceneMetadata {
        name: Some(name.into()),
        comment: None,
    })
}

fn moved(x: f64) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0 / 3.0]],
        [x, -0.0, 1e-300],
    ))
}

/// Every node kind: a shared mesh resource, an inline mesh shared by two
/// leaves, a group, a CSG node, and a subtree resource referenced twice.
pub(crate) fn scene() -> Scene {
    let mut scene = Scene::new();
    scene.set_meta(Some(SceneMetadata {
        name: Some("plate".into()),
        comment: Some("two wolves".into()),
    }));
    let resource = scene.add_mesh_resource(grid(3));
    let inline = Arc::new(grid(2));

    let a = scene
        .add_leaf(named("a"), moved(1.0), LeafPayload::MeshResource(resource))
        .unwrap();
    let b = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::clone(&inline)),
        )
        .unwrap();
    let group = scene.add_group(named("group"), moved(2.0));
    scene.add_child(group, b).unwrap();
    scene.add_child(group, a).unwrap();

    let c = scene
        .add_leaf(None, moved(3.0), LeafPayload::Mesh(Arc::clone(&inline)))
        .unwrap();
    let d = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::MeshResource(resource),
        )
        .unwrap();
    let csg = scene
        .add_csg(
            named("cut"),
            Transform3D::Identity,
            CsgOp::Difference,
            vec![c, d],
        )
        .unwrap();
    let subtree = scene.register_subtree_resource(csg).unwrap();
    let first = scene
        .add_reference(named("first"), moved(4.0), subtree)
        .unwrap();
    let second = scene
        .add_reference(None, Transform3D::Identity, subtree)
        .unwrap();

    scene.add_root(first).unwrap();
    scene.add_root(group).unwrap();
    scene.add_root(second).unwrap();
    scene
}

#[test]
fn a_scene_comes_back_with_its_graph_and_shared_meshes() {
    let scene = scene();
    let encoded = encode_scene(&scene).unwrap();
    assert_eq!(encoded.meshes.len(), 2, "each mesh is stored once");
    let back = decode_scene(&encoded.payload, encoded.meshes.clone()).unwrap();

    // The encoding is canonical, so equal payloads mean equal scenes.
    let again = encode_scene(&back).unwrap();
    assert_eq!(again.payload, encoded.payload);
    assert_eq!(back.meta(), scene.meta());

    let names = |s: &Scene| {
        s.roots()
            .iter()
            .map(|&r| s.node(r).unwrap().meta.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&back), names(&scene));

    // The inline mesh is still one mesh behind both leaves.
    let mut inline = Vec::new();
    for root in back.roots() {
        if let NodeKind::Group { children } = &back.node(*root).unwrap().kind {
            for child in children {
                if let NodeKind::Leaf(LeafPayload::Mesh(arc)) = &back.node(*child).unwrap().kind {
                    inline.push(Arc::clone(arc));
                }
            }
        }
    }
    assert_eq!(inline.len(), 1);
    assert!(encoded.meshes.iter().any(|m| Arc::ptr_eq(m, &inline[0])));
}

#[test]
fn transforms_come_back_bit_for_bit() {
    let scene = scene();
    let encoded = encode_scene(&scene).unwrap();
    let back = decode_scene(&encoded.payload, encoded.meshes).unwrap();
    let transform = |s: &Scene| s.node(s.roots()[0]).unwrap().local_transform;
    let Transform3D::Affine(affine) = transform(&back) else {
        panic!("the first root is moved");
    };
    assert_eq!(Transform3D::Affine(affine), transform(&scene));
    assert_eq!(affine.translation()[1].to_bits(), (-0.0f64).to_bits());
}

#[test]
fn an_empty_scene_round_trips() {
    let encoded = encode_scene(&Scene::new()).unwrap();
    assert!(encoded.meshes.is_empty());
    let back = decode_scene(&encoded.payload, vec![]).unwrap();
    assert!(back.roots().is_empty());
    assert_eq!(back.meta(), &None);
}

#[test]
fn a_missing_mesh_is_reported() {
    let encoded = encode_scene(&scene()).unwrap();
    let mut meshes = encoded.meshes;
    meshes.pop();
    assert_eq!(
        decode_scene(&encoded.payload, meshes).unwrap_err(),
        DecodeSceneError::MeshCount { named: 2, found: 1 }
    );
}

#[test]
fn a_cut_or_padded_payload_is_refused() {
    let encoded = encode_scene(&scene()).unwrap();
    let payload = &encoded.payload;
    for cut in 0..payload.len() {
        assert!(
            decode_scene(&payload[..cut], encoded.meshes.clone()).is_err(),
            "cut at {cut}"
        );
    }
    let mut padded = payload.clone();
    padded.push(0);
    assert_eq!(
        decode_scene(&padded, encoded.meshes).unwrap_err(),
        DecodeSceneError::Malformed
    );
}

#[test]
fn a_record_cannot_name_a_node_listed_after_it() {
    let mut payload = vec![0u8]; // no metadata
    payload.extend_from_slice(&0u32.to_le_bytes()); // meshes
    payload.extend_from_slice(&0u32.to_le_bytes()); // resources
    payload.extend_from_slice(&1u32.to_le_bytes()); // one record:
    payload.extend_from_slice(&[0, 0, 0]); // a group, no metadata, identity
    payload.extend_from_slice(&1u32.to_le_bytes()); // with one child,
    payload.extend_from_slice(&0u32.to_le_bytes()); // itself
    payload.extend_from_slice(&0u32.to_le_bytes()); // no roots
    assert_eq!(
        decode_scene(&payload, vec![]).unwrap_err(),
        DecodeSceneError::Malformed
    );
}
