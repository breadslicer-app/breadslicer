/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::indexed_mesh::{IndexedMesh, MeshMetadata};
use sc_mesh_core::transform::Transform3D;
use sc_scene::{LeafPayload, NodeId, NodeKind, Scene};

use crate::container::{BreadReader, BreadWriter};
use crate::file::{BreadProject, ReadProjectError, WriteBreadError, read_project, write_project};
use crate::mesh::{MESH_TAG, MESH_VERSION, encode_mesh};
use crate::scene::{SCENE_TAG, decode_scene, encode_scene};
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
        surface_notes: Vec::new(),
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
        surface_notes: Vec::new(),
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
        surface_notes: Vec::new(),
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

/// The test scene plus a root leaf that shows supports through a mesh
/// resource, as the application adds them.
fn with_support_resource() -> (Scene, NodeId) {
    let mut scene = scene();
    let resource = scene.add_mesh_resource(grid(30));
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::MeshResource(resource),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    (scene, leaf)
}

fn stored_vertices(bytes: &[u8]) -> usize {
    let back = read_project(bytes).unwrap();
    back.scene
        .mesh_resources_iter()
        .map(|(_, mesh)| mesh.vertices().len())
        .sum()
}

#[test]
fn a_support_mesh_resource_is_not_stored() {
    let (scene, leaf) = with_support_resource();
    let project = BreadProject {
        scene,
        supports: vec![document(leaf)],
        ..Default::default()
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let without = write_project(
        &BreadProject {
            scene: crate::tests_scene::scene(),
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap();
    let back = read_project(bytes.as_slice()).unwrap();
    // Only the test scene's own resource comes back.
    assert_eq!(back.scene.mesh_resources_iter().count(), 1);
    assert_eq!(stored_vertices(&bytes), stored_vertices(&without));
    let leaf = back.supports[0].leaf;
    assert!(leaf_mesh(&back.scene, leaf).vertices().is_empty());

    // Opening and saving again stores the same file: nothing piles up.
    let again = write_project(&back, Vec::new()).unwrap();
    assert_eq!(again, bytes);
}

#[test]
fn a_resource_also_shown_by_a_model_leaf_is_kept() {
    let (mut scene, leaf) = with_support_resource();
    let NodeKind::Leaf(LeafPayload::MeshResource(resource)) = scene.node(leaf).unwrap().kind else {
        unreachable!()
    };
    let model = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::MeshResource(resource),
        )
        .unwrap();
    scene.add_root(model).unwrap();
    let project = BreadProject {
        scene,
        supports: vec![document(leaf)],
        ..Default::default()
    };
    let back = read_project(write_project(&project, Vec::new()).unwrap().as_slice()).unwrap();
    assert_eq!(back.scene.mesh_resources_iter().count(), 2);
    let model = *back.scene.roots().last().unwrap();
    let NodeKind::Leaf(LeafPayload::MeshResource(resource)) = back.scene.node(model).unwrap().kind
    else {
        panic!("the model leaf still shows a resource")
    };
    let mesh = back.scene.mesh_resource(resource).unwrap();
    assert_eq!(mesh.vertices().len(), grid(30).vertices().len());
}

/// `bytes` as a build before #220 wrote it: plus a mesh resource, listed
/// last, that no leaf shows. The scene must have no metadata.
fn with_leftover_resource(bytes: &[u8], mesh: &IndexedMesh) -> Vec<u8> {
    let field = |payload: &[u8], at: usize| {
        u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()) as usize
    };
    let mut reader = BreadReader::new(bytes).unwrap();
    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    while let Some(mut chunk) = reader.next_chunk().unwrap() {
        if chunk.tag == SCENE_TAG {
            // metadata (u8 0), meshes, resources, then one u32 per resource.
            let payload = &mut chunk.payload;
            assert_eq!(payload[0], 0, "no scene metadata");
            let (meshes, resources) = (field(payload, 1), field(payload, 5));
            payload[1..5].copy_from_slice(&(meshes as u32 + 1).to_le_bytes());
            payload[5..9].copy_from_slice(&(resources as u32 + 1).to_le_bytes());
            let end = 9 + 4 * resources;
            payload.splice(end..end, (meshes as u32).to_le_bytes());
        }
        writer
            .chunk(chunk.tag, chunk.version, &chunk.payload)
            .unwrap();
    }
    writer
        .chunk(MESH_TAG, MESH_VERSION, &encode_mesh(mesh))
        .unwrap();
    writer.finish().unwrap()
}

#[test]
fn a_file_with_a_leftover_support_mesh_drops_it_on_save() {
    let (mut scene, leaf) = with_supports();
    scene.set_meta(None);
    let project = BreadProject {
        scene,
        supports: vec![document(leaf)],
        ..Default::default()
    };
    let current = write_project(&project, Vec::new()).unwrap();
    let old = with_leftover_resource(&current, &grid(30));
    let opened = read_project(old.as_slice()).unwrap();
    assert_eq!(
        opened.scene.mesh_resources_iter().count(),
        2,
        "the leftover mesh comes back as a resource"
    );

    // Saving drops it: the file is the one this build writes, and stays so.
    let saved = write_project(&opened, Vec::new()).unwrap();
    assert_eq!(saved, current);
    let back = read_project(saved.as_slice()).unwrap();
    assert_eq!(back.scene.mesh_resources_iter().count(), 1);
    assert_eq!(back.supports, opened.supports);
    assert_eq!(write_project(&back, Vec::new()).unwrap(), saved);
}
