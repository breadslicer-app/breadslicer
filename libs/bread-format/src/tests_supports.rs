/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::indexed_mesh::{IndexedMesh, MeshMetadata};
use sc_mesh_core::transform::Transform3D;
use sc_scene::{LeafPayload, NodeId, NodeKind, Scene};

use crate::file::{BreadProject, ReadProjectError, WriteBreadError, read_project, write_project};
use crate::scene::{decode_scene, encode_scene};
use crate::supports::{
    DecodeSupportsError, SUPPORTS_TAG, SupportDocument, decode_supports, encode_supports,
};
use crate::tests_file::rewrite;
use crate::tests_mesh::grid;
use crate::tests_scene::scene;

/// The test scene plus a root leaf whose mesh is named like supports.
fn with_supports() -> (Scene, NodeId) {
    let mut scene = scene();
    let mut mesh = grid(30);
    mesh.set_meta(Some(MeshMetadata {
        name: Some("Supports".into()),
        header_comment: None,
        declared_facet_normals: None,
    }));
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    (scene, leaf)
}

fn document(leaf: NodeId) -> SupportDocument {
    SupportDocument {
        leaf,
        generator: "df".into(),
        version: 3,
        document: br#"{"roots":[{"tip":{"x":1,"y":2,"z":3}}]}"#.to_vec(),
    }
}

fn leaf_mesh(scene: &Scene, id: NodeId) -> Arc<IndexedMesh> {
    match &scene.node(id).unwrap().kind {
        NodeKind::Leaf(LeafPayload::Mesh(mesh)) => Arc::clone(mesh),
        kind => panic!("not an inline leaf: {kind:?}"),
    }
}

fn is_leaf(scene: &Scene) -> impl Fn(NodeId) -> bool + '_ {
    |id| matches!(scene.node(id).map(|n| &n.kind), Some(NodeKind::Leaf(_)))
}

#[test]
fn supports_are_stored_as_data_and_their_mesh_left_out() {
    let (scene, leaf) = with_supports();
    let with_mesh = write_project(
        &BreadProject {
            scene: scene.clone(),
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap();
    let project = BreadProject {
        scene,
        settings: None,
        supports: vec![document(leaf)],
        hollows: Vec::new(),
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    assert!(
        bytes.len() < with_mesh.len(),
        "the support mesh is not stored"
    );

    let back = read_project(bytes.as_slice()).unwrap();
    let [support] = back.supports.as_slice() else {
        panic!("one support document");
    };
    assert_eq!(
        *support,
        SupportDocument {
            leaf: support.leaf,
            ..document(leaf)
        }
    );
    // The leaf comes back in place, empty but still named as supports, for
    // the application to rebuild.
    let mesh = leaf_mesh(&back.scene, support.leaf);
    assert!(mesh.vertices().is_empty() && mesh.faces().is_empty());
    assert_eq!(
        mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
        Some("Supports")
    );
    assert_eq!(back.scene.roots().last(), Some(&support.leaf));
}

#[test]
fn supports_of_a_part_outside_the_scene_are_refused() {
    let (mut scene, _) = with_supports();
    let loose = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(grid(1))),
        )
        .unwrap();
    let project = BreadProject {
        scene,
        settings: None,
        supports: vec![document(loose)],
        hollows: Vec::new(),
    };
    assert!(matches!(
        write_project(&project, Vec::new()),
        Err(WriteBreadError::SupportLeafMissing)
    ));
}

#[test]
fn supports_must_name_a_leaf() {
    let (scene, _) = with_supports();
    let encoded = encode_scene(&scene).unwrap();
    let group = scene.roots()[1];
    let payload =
        encode_supports(&[document(group)], |id| encoded.nodes.get(&id).copied()).unwrap();
    let decoded = decode_scene(&encoded.payload, encoded.meshes).unwrap();
    assert_eq!(
        decode_supports(&payload, &decoded.nodes, is_leaf(&decoded.scene)),
        Err(DecodeSupportsError::NotALeaf)
    );
}

#[test]
fn a_damaged_supports_chunk_is_reported() {
    let (scene, leaf) = with_supports();
    let project = BreadProject {
        scene,
        settings: None,
        supports: vec![document(leaf)],
        hollows: Vec::new(),
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let cut = rewrite(&bytes, |tag, version, mut payload| {
        if tag == SUPPORTS_TAG {
            payload.pop();
        }
        vec![(tag, version, payload)]
    });
    assert!(matches!(
        read_project(cut.as_slice()),
        Err(ReadProjectError::Supports(DecodeSupportsError::Malformed))
    ));
}
