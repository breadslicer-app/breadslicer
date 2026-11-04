/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::container::{BreadReader, BreadWriter, ChunkTag};
use crate::file::{BreadProject, ReadProjectError, read_project, write_project};
use crate::scene::{SCENE_TAG, SCENE_VERSION, encode_scene};
use crate::tests_scene::scene;

fn project() -> BreadProject {
    BreadProject {
        scene: scene(),
        settings: Some(b"version = 1\nprinter = \"m7-pro\"\n".to_vec()),
        supports: vec![],
        hollows: vec![],
        surface_notes: Vec::new(),
    }
}

#[test]
fn a_project_survives_the_file() {
    let project = project();
    let bytes = write_project(&project, Vec::new()).unwrap();
    let back = read_project(bytes.as_slice()).unwrap();
    assert_eq!(
        encode_scene(&back.scene).unwrap().payload,
        encode_scene(&project.scene).unwrap().payload
    );
    assert_eq!(back.settings, project.settings);
    assert!(back.supports.is_empty());
}

#[test]
fn a_project_without_settings_has_none() {
    let mut project = project();
    project.settings = None;
    let bytes = write_project(&project, Vec::new()).unwrap();
    assert_eq!(read_project(bytes.as_slice()).unwrap().settings, None);
}

/// The file's chunks, rewritten through `edit`.
pub(crate) fn rewrite(
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
    let bytes = write_project(&project(), Vec::new()).unwrap();
    let extended = rewrite(&bytes, |tag, version, payload| {
        vec![
            (ChunkTag(*b"NEW1"), 9, b"from a newer build".to_vec()),
            (tag, version, payload),
        ]
    });
    assert!(read_project(extended.as_slice()).is_ok());
}

#[test]
fn a_newer_scene_layout_is_refused() {
    let bytes = write_project(&project(), Vec::new()).unwrap();
    let newer = rewrite(&bytes, |tag, version, payload| {
        let version = if tag == SCENE_TAG {
            SCENE_VERSION + 1
        } else {
            version
        };
        vec![(tag, version, payload)]
    });
    assert!(matches!(
        read_project(newer.as_slice()),
        Err(ReadProjectError::NewerChunk { tag, .. }) if tag == SCENE_TAG
    ));
}

#[test]
fn a_file_without_a_scene_or_with_two_is_refused() {
    let bytes = write_project(&project(), Vec::new()).unwrap();
    let headless = rewrite(&bytes, |tag, version, payload| {
        if tag == SCENE_TAG {
            vec![]
        } else {
            vec![(tag, version, payload)]
        }
    });
    assert!(matches!(
        read_project(headless.as_slice()),
        Err(ReadProjectError::ChunkCount { tag, count: 0 }) if tag == SCENE_TAG
    ));
    let doubled = rewrite(&bytes, |tag, version, payload| {
        let copies = if tag == SCENE_TAG { 2 } else { 1 };
        vec![(tag, version, payload); copies]
    });
    assert!(matches!(
        read_project(doubled.as_slice()),
        Err(ReadProjectError::ChunkCount { count: 2, .. })
    ));
}

#[test]
fn a_thumbnail_reads_back_and_a_file_without_one_has_none() {
    use crate::container::{BreadWriter, ChunkTag};
    use crate::file::{THUMBNAIL_TAG, read_thumbnail};

    let png = b"\x89PNG\r\n\x1a\n a picture".to_vec();
    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    writer.chunk(ChunkTag(*b"SCNE"), 1, &[0; 2048]).unwrap();
    writer.chunk(THUMBNAIL_TAG, 1, &png).unwrap();
    let with = writer.finish().unwrap();
    assert_eq!(
        read_thumbnail(std::io::Cursor::new(&with)).unwrap(),
        Some(png)
    );

    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    writer.chunk(ChunkTag(*b"SCNE"), 1, &[0; 16]).unwrap();
    let without = writer.finish().unwrap();
    assert_eq!(
        read_thumbnail(std::io::Cursor::new(&without)).unwrap(),
        None
    );
}
