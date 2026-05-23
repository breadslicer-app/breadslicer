/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod stla;
mod stlb;
mod writers;

use std::io::{Read, Seek};

use sc_mesh_core::Scene;
use sc_mesh_core::TriangleIterator;

use crate::error::LoadStlError;
use crate::messages::LoadStlWarning;
use crate::ports::{MeshReader, sealed};
use stla::AsciiStlReader;
use stlb::BinaryStlReader;

pub use writers::{write_stla, write_stlb};

type Result<T> = std::result::Result<T, LoadStlError>;

/// Format marker for the binary STL format. Implements
/// [`MeshReader`] via static dispatch; the dispatcher in
/// [`crate::loader`] invokes `StlBin::read(reader)`.
pub struct StlBin;

impl sealed::Sealed for StlBin {}

impl MeshReader for StlBin {
    type Error = LoadStlError;
    type Warning = LoadStlWarning;

    fn read<R: Read + Seek>(reader: &mut R) -> Result<(Scene, Vec<LoadStlWarning>)> {
        read_stlb(reader).map(|scene| (scene, Vec::new()))
    }
}

/// Format marker for the ASCII STL format. Like [`StlBin`], it
/// implements [`MeshReader`] via static dispatch.
pub struct StlAscii;

impl sealed::Sealed for StlAscii {}

impl MeshReader for StlAscii {
    type Error = LoadStlError;
    type Warning = LoadStlWarning;

    fn read<R: Read + Seek>(reader: &mut R) -> Result<(Scene, Vec<LoadStlWarning>)> {
        read_stla(reader).map(|scene| (scene, Vec::new()))
    }
}

/// Format marker for the auto-detecting STL reader. Probes the first
/// bytes of `reader` to decide between ASCII and binary, then
/// dispatches internally. Use [`StlBin`] or [`StlAscii`] directly when
/// the variant is known.
pub struct Stl;

impl sealed::Sealed for Stl {}

impl MeshReader for Stl {
    type Error = LoadStlError;
    type Warning = LoadStlWarning;

    fn read<R: Read + Seek>(reader: &mut R) -> Result<(Scene, Vec<LoadStlWarning>)> {
        read_stl(reader).map(|scene| (scene, Vec::new()))
    }
}

pub fn read_stl<R>(read: &mut R) -> Result<Scene>
where
    R: std::io::Read + std::io::Seek,
{
    let is_ascii = AsciiStlReader::probe(read)?;

    let metadata = if is_ascii {
        AsciiStlReader::extract_metadata(read)
    } else {
        BinaryStlReader::extract_metadata(read)
    }?;

    let mut stl_reader = if is_ascii {
        AsciiStlReader::create_triangles_iterator(read)
    } else {
        BinaryStlReader::create_triangles_iterator(read)
    }?;

    let mesh = stl_reader.as_indexed_mesh(Some(metadata))?;
    Ok(Scene::from(mesh))
}

pub fn read_stla<R>(read: &mut R) -> Result<Scene>
where
    R: std::io::Read + std::io::Seek,
{
    let metadata = AsciiStlReader::extract_metadata(read)?;
    let mesh = AsciiStlReader::create_triangles_iterator(read)?.as_indexed_mesh(Some(metadata))?;
    Ok(Scene::from(mesh))
}

pub fn read_stlb<R>(read: &mut R) -> Result<Scene>
where
    R: std::io::Read + std::io::Seek,
{
    let metadata = BinaryStlReader::extract_metadata(read)?;
    let mesh = BinaryStlReader::create_triangles_iterator(read)?.as_indexed_mesh(Some(metadata))?;
    Ok(Scene::from(mesh))
}

#[cfg(test)]
mod stl_bin_reader_tests {
    use std::io::Cursor;

    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};

    use super::{MeshReader, StlBin, write_stlb};

    fn one_triangle_buffer() -> Vec<u8> {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([-1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        };
        let mut buf = Vec::new();
        write_stlb(&mesh, &mut buf).expect("write_stlb succeeds");
        buf
    }

    #[test]
    fn read_returns_a_one_leaf_scene_with_no_warnings() {
        let mut reader = Cursor::new(one_triangle_buffer());
        let (scene, warnings) = StlBin::read(&mut reader).expect("StlBin::read succeeds");

        assert!(warnings.is_empty());
        assert_eq!(scene.reachable_leaf_count(), 1);
        let leaf = scene
            .first_leaf_id()
            .expect("reachable_leaf_count == 1 implies a leaf exists");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);
    }
}
