/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_scene::{LeafPayload, NodeId, NodeKind, Scene};

use crate::file::{BreadProject, ReadProjectError, WriteBreadError, read_project, write_project};
use crate::hollows::{
    DecodeHollowsError, HOLLOWS_TAG, HollowDocument, decode_hollows, encode_hollows,
};
use crate::scene::{decode_scene, encode_scene};
use crate::tests_file::rewrite;
use crate::tests_mesh::grid;
use crate::tests_scene::scene;

/// The test scene plus a root leaf standing for a hollowed model.
fn with_hollow() -> (Scene, NodeId) {
    let mut scene = scene();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(grid(30))),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    (scene, leaf)
}

fn document(leaf: NodeId) -> HollowDocument {
    HollowDocument {
        leaf,
        hollower: "df".into(),
        version: 1,
        document: b"original mesh, settings, drains".to_vec(),
    }
}

fn faces(scene: &Scene, id: NodeId) -> usize {
    match &scene.node(id).unwrap().kind {
        NodeKind::Leaf(LeafPayload::Mesh(mesh)) => mesh.faces().len(),
        kind => panic!("not an inline leaf: {kind:?}"),
    }
}

fn is_leaf(scene: &Scene) -> impl Fn(NodeId) -> bool + '_ {
    |id| matches!(scene.node(id).map(|n| &n.kind), Some(NodeKind::Leaf(_)))
}

#[test]
fn a_hollow_is_stored_with_its_mesh_and_its_document() {
    let (scene, leaf) = with_hollow();
    let project = BreadProject {
        scene,
        settings: None,
        supports: Vec::new(),
        hollows: vec![document(leaf)],
        surface_notes: Vec::new(),
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let back = read_project(bytes.as_slice()).unwrap();
    let [hollow] = back.hollows.as_slice() else {
        panic!("one hollow document");
    };
    assert_eq!(
        *hollow,
        HollowDocument {
            leaf: hollow.leaf,
            ..document(leaf)
        }
    );
    // Unlike supports, the leaf keeps its mesh: the hollow itself.
    assert_eq!(faces(&back.scene, hollow.leaf), faces(&project.scene, leaf));
    assert_eq!(back.scene.roots().last(), Some(&hollow.leaf));
}

#[test]
fn a_file_without_hollows_reads_none() {
    let (scene, _) = with_hollow();
    let bytes = write_project(
        &BreadProject {
            scene,
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap();
    assert!(read_project(bytes.as_slice()).unwrap().hollows.is_empty());
}

#[test]
fn a_build_that_does_not_know_hollows_opens_the_hollow() {
    // Without the hollows chunk, as an older build reads the file, the model
    // is still there, hollow.
    let (scene, leaf) = with_hollow();
    let project = BreadProject {
        scene,
        settings: None,
        supports: Vec::new(),
        hollows: vec![document(leaf)],
        surface_notes: Vec::new(),
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let older = rewrite(&bytes, |tag, version, payload| {
        if tag == HOLLOWS_TAG {
            Vec::new()
        } else {
            vec![(tag, version, payload)]
        }
    });
    let back = read_project(older.as_slice()).unwrap();
    assert!(back.hollows.is_empty());
    let last = *back.scene.roots().last().unwrap();
    assert_eq!(faces(&back.scene, last), faces(&project.scene, leaf));
}

#[test]
fn hollows_of_a_part_outside_the_scene_are_refused() {
    let (mut scene, _) = with_hollow();
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
        supports: Vec::new(),
        hollows: vec![document(loose)],
        surface_notes: Vec::new(),
    };
    assert!(matches!(
        write_project(&project, Vec::new()),
        Err(WriteBreadError::HollowLeafMissing)
    ));
}

#[test]
fn hollows_must_name_a_leaf() {
    let (scene, _) = with_hollow();
    let encoded = encode_scene(&scene).unwrap();
    let group = scene.roots()[1];
    let payload = encode_hollows(&[document(group)], |id| encoded.nodes.get(&id).copied()).unwrap();
    let decoded = decode_scene(&encoded.payload, encoded.meshes).unwrap();
    assert_eq!(
        decode_hollows(&payload, &decoded.nodes, is_leaf(&decoded.scene)),
        Err(DecodeHollowsError::NotALeaf)
    );
}

#[test]
fn a_damaged_hollows_chunk_is_reported() {
    let (scene, leaf) = with_hollow();
    let project = BreadProject {
        scene,
        settings: None,
        supports: Vec::new(),
        hollows: vec![document(leaf)],
        surface_notes: Vec::new(),
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let cut = rewrite(&bytes, |tag, version, mut payload| {
        if tag == HOLLOWS_TAG {
            payload.pop();
        }
        vec![(tag, version, payload)]
    });
    assert!(matches!(
        read_project(cut.as_slice()),
        Err(ReadProjectError::Hollows(DecodeHollowsError::Malformed))
    ));
}
