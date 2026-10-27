/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::container::{BreadReader, BreadWriter, ChunkTag};
use crate::file::{ReadSceneError, read_scene, write_scene};
use crate::scene::{SCENE_TAG, SCENE_VERSION, encode_scene};
use crate::tests_scene::scene;

#[test]
fn a_scene_survives_the_file() {
    let scene = scene();
    let bytes = write_scene(&scene, Vec::new()).unwrap();
    let back = read_scene(bytes.as_slice()).unwrap();
    assert_eq!(
        encode_scene(&back).unwrap().payload,
        encode_scene(&scene).unwrap().payload
    );
}

/// The file's chunks, rewritten through `edit`.
fn rewrite(
    bytes: &[u8],
    edit: impl Fn(ChunkTag, u16, Vec<u8>) -> Vec<(ChunkTag, u16, Vec<u8>)>,
) -> Vec<u8> {
    let mut reader = BreadReader::new(bytes).unwrap();
    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    while let Some(chunk) = reader.next_chunk().unwrap() {
        for (tag, version, payload) in edit(chunk.tag, chunk.version, chunk.payload) {
            writer.chunk(tag, version, &payload).unwrap();
        }
    }
    writer.finish().unwrap()
}

#[test]
fn unknown_chunks_are_skipped() {
    let bytes = write_scene(&scene(), Vec::new()).unwrap();
    let extended = rewrite(&bytes, |tag, version, payload| {
        vec![
            (ChunkTag(*b"NEW1"), 9, b"from a newer build".to_vec()),
            (tag, version, payload),
        ]
    });
    assert!(read_scene(extended.as_slice()).is_ok());
}

#[test]
fn a_newer_scene_layout_is_refused() {
    let bytes = write_scene(&scene(), Vec::new()).unwrap();
    let newer = rewrite(&bytes, |tag, version, payload| {
        let version = if tag == SCENE_TAG {
            SCENE_VERSION + 1
        } else {
            version
        };
        vec![(tag, version, payload)]
    });
    assert!(matches!(
        read_scene(newer.as_slice()),
        Err(ReadSceneError::NewerChunk { tag, .. }) if tag == SCENE_TAG
    ));
}

#[test]
fn a_file_without_a_scene_is_refused() {
    let bytes = write_scene(&scene(), Vec::new()).unwrap();
    let headless = rewrite(&bytes, |tag, version, payload| {
        if tag == SCENE_TAG {
            vec![]
        } else {
            vec![(tag, version, payload)]
        }
    });
    assert!(matches!(
        read_scene(headless.as_slice()),
        Err(ReadSceneError::SceneChunkCount(0))
    ));
}
