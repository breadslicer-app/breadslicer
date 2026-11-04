/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_scene::{LeafPayload, NodeId, PAINTED, PaintChanges, PaintKind, Scene, UNPAINTED};

use crate::file::{BreadProject, read_project, write_project};
use crate::surface::{SURFACE_TAG, SurfaceNote};
use crate::tests_file::rewrite;
use crate::tests_mesh::grid;
use crate::tests_scene::scene;

/// The test scene plus a root leaf with a grid mesh.
fn with_model() -> (Scene, NodeId) {
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

fn paint(scene: &mut Scene, leaf: NodeId, kind: PaintKind, faces: std::ops::Range<u32>) {
    let changes = PaintChanges::from_faces(faces.map(|f| (f, UNPAINTED, PAINTED)).collect());
    scene.paint_surface(leaf, kind, &changes).unwrap();
}

fn saved(scene: Scene) -> Vec<u8> {
    let project = BreadProject {
        scene,
        ..BreadProject::default()
    };
    write_project(&project, Vec::new()).unwrap()
}

/// The file with its paint chunk's version and payload changed by `edit`.
fn with_surface_edited(bytes: &[u8], edit: impl Fn(&mut u16, &mut Vec<u8>)) -> Vec<u8> {
    rewrite(bytes, |tag, mut version, mut payload| {
        if tag == SURFACE_TAG {
            edit(&mut version, &mut payload);
        }
        vec![(tag, version, payload)]
    })
}

/// The stored leaf that shows the grid, as read back.
fn model_of(project: &BreadProject) -> NodeId {
    project
        .scene
        .surface_tags_iter()
        .map(|(leaf, _, _)| leaf)
        .next()
        .expect("the paint came back")
}

#[test]
fn paint_comes_back_on_the_same_faces_for_each_kind() {
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 3..40);
    paint(&mut scene, leaf, PaintKind::SupportBlock, 100..103);
    let expected: Vec<u8> = scene
        .surface_tags(leaf, PaintKind::Detail)
        .unwrap()
        .values()
        .to_vec();
    let back = read_project(saved(scene).as_slice()).unwrap();
    assert!(back.surface_notes.is_empty());
    let leaf = model_of(&back);
    let detail = back.scene.surface_tags(leaf, PaintKind::Detail).unwrap();
    assert_eq!(detail.values(), expected);
    assert_eq!(detail.painted_faces(), 37);
    let block = back.scene.surface_tags(leaf, PaintKind::SupportBlock);
    assert_eq!(block.unwrap().painted_faces(), 3);
    assert!(
        back.scene
            .surface_tags(leaf, PaintKind::SupportEnforce)
            .is_none()
    );
}

#[test]
fn a_scene_without_paint_writes_no_chunk() {
    let (scene, _) = with_model();
    let bytes = saved(scene);
    let found = std::cell::Cell::new(false);
    rewrite(&bytes, |tag, version, payload| {
        found.set(found.get() || tag == SURFACE_TAG);
        vec![(tag, version, payload)]
    });
    assert!(!found.get());
}

#[test]
fn a_mask_stays_small_on_disk() {
    let (mut scene, leaf) = with_model();
    let without = saved(scene.clone()).len();
    paint(&mut scene, leaf, PaintKind::Detail, 10..600);
    let with = saved(scene).len();
    assert!(with - without < 200, "{} bytes", with - without);
}

#[test]
fn a_build_that_does_not_know_the_chunk_opens_the_file() {
    // What an older build does with a chunk it does not know: it reads the
    // file as if the chunk were not there.
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 0..4);
    let mut plain = scene.clone();
    plain
        .set_surface_tags(leaf, PaintKind::Detail, None)
        .unwrap();
    let bytes = saved(scene);
    let stripped = rewrite(&bytes, |tag, version, payload| {
        if tag == SURFACE_TAG {
            Vec::new()
        } else {
            vec![(tag, version, payload)]
        }
    });
    let back = read_project(stripped.as_slice()).unwrap();
    assert_eq!(back.scene.surface_tags_iter().count(), 0);
    assert_eq!(back.scene.roots().len(), plain.roots().len());
}

#[test]
fn a_newer_chunk_opens_the_file_without_the_paint_and_says_so() {
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 0..4);
    let bytes = saved(scene);
    let newer = with_surface_edited(&bytes, |version, _| *version = 99);
    let back = read_project(newer.as_slice()).unwrap();
    assert_eq!(back.scene.surface_tags_iter().count(), 0);
    assert_eq!(
        back.surface_notes,
        [SurfaceNote::NewerChunk { version: 99 }]
    );
}

#[test]
fn damaged_paint_opens_the_file_without_it_and_says_so() {
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 0..4);
    let bytes = saved(scene);
    let cut = with_surface_edited(&bytes, |_, payload| payload.truncate(payload.len() - 3));
    let back = read_project(cut.as_slice()).unwrap();
    assert_eq!(back.scene.surface_tags_iter().count(), 0);
    assert_eq!(back.surface_notes, [SurfaceNote::Malformed]);
}

#[test]
fn paint_for_another_mesh_is_left_out_and_said() {
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 0..4);
    let bytes = saved(scene);
    // One bit of the stored fingerprint, after the entry's header: leaf
    // index, kind, version, document length and face count.
    let tampered = with_surface_edited(&bytes, |_, payload| payload[4 + 4 + 1 + 4 + 4 + 4] ^= 1);
    let back = read_project(tampered.as_slice()).unwrap();
    assert_eq!(back.scene.surface_tags_iter().count(), 0);
    assert_eq!(back.surface_notes, [SurfaceNote::MeshChanged]);
}

#[test]
fn an_unknown_kind_leaves_out_that_entry_alone() {
    let (mut scene, leaf) = with_model();
    paint(&mut scene, leaf, PaintKind::Detail, 0..4);
    paint(&mut scene, leaf, PaintKind::SupportBlock, 8..9);
    let bytes = saved(scene);
    // The first entry's kind, after the count and the leaf index.
    let unknown = with_surface_edited(&bytes, |_, payload| payload[8] = 200);
    let back = read_project(unknown.as_slice()).unwrap();
    assert_eq!(back.surface_notes, [SurfaceNote::UnknownKind { code: 200 }]);
    assert_eq!(back.scene.surface_tags_iter().count(), 1);
}

#[test]
fn copies_sharing_a_mesh_keep_each_their_paint() {
    let mut scene = Scene::new();
    let mesh = Arc::new(grid(30));
    let rid = scene.add_mesh_resource_arc(mesh);
    let mut leaves = Vec::new();
    for _ in 0..2 {
        let leaf = scene
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        scene.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    paint(&mut scene, leaves[0], PaintKind::Detail, 0..3);
    paint(&mut scene, leaves[1], PaintKind::Detail, 7..8);
    let back = read_project(saved(scene).as_slice()).unwrap();
    let counts: Vec<usize> = back
        .scene
        .surface_tags_iter()
        .map(|(_, _, tags)| tags.painted_faces())
        .collect();
    assert_eq!(counts, [3, 1]);
    let meshes: Vec<_> = back
        .scene
        .surface_tags_iter()
        .map(|(_, _, tags)| Arc::as_ptr(tags.mesh()))
        .collect();
    assert_eq!(meshes[0], meshes[1]);
}

#[test]
fn the_writer_reads_back_a_chunk_written_by_hand() {
    // Nothing but the layout documented in `surface.rs`.
    let (scene, _) = with_model();
    let project = BreadProject {
        scene,
        ..BreadProject::default()
    };
    let bytes = write_project(&project, Vec::new()).unwrap();
    let back = read_project(bytes.as_slice()).unwrap();
    let mesh = back
        .scene
        .leaf_mesh_arc(*back.scene.roots().last().unwrap())
        .unwrap()
        .clone();
    let faces = mesh.faces().len() as u32;
    let mut document = Vec::new();
    document.extend_from_slice(&faces.to_le_bytes());
    document.extend_from_slice(mesh.geometry_fingerprint().as_ref());
    document.extend_from_slice(&2u32.to_le_bytes());
    document.extend_from_slice(&1u32.to_le_bytes());
    document.push(255);
    document.extend_from_slice(&(faces - 1).to_le_bytes());
    document.push(0);
    let index = crate::scene::encode_scene(&back.scene)
        .unwrap()
        .nodes
        .get(back.scene.roots().last().unwrap())
        .copied()
        .unwrap() as u32;
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&index.to_le_bytes());
    payload.push(0);
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&(document.len() as u32).to_le_bytes());
    payload.extend_from_slice(&document);
    let by_hand = rewrite(&bytes, |tag, version, chunk| {
        let mut out = vec![(tag, version, chunk)];
        if tag == crate::scene::SCENE_TAG {
            out.push((SURFACE_TAG, 1, payload.clone()));
        }
        out
    });
    let read = read_project(by_hand.as_slice()).unwrap();
    assert!(read.surface_notes.is_empty(), "{:?}", read.surface_notes);
    let (_, kind, tags) = read.scene.surface_tags_iter().next().unwrap();
    assert_eq!(kind, PaintKind::Detail);
    assert_eq!(tags.painted_faces(), 1);
    assert_eq!(tags.values()[0], PAINTED);
}
