/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod stla;
mod stlb;
mod writers;

use anyhow::Result;

use stla::AsciiStlReader;
use stlb::BinaryStlReader;

use crate::core::{IndexedMesh, MeshMetadata, TriangleIterator};

pub use writers::{write_stla, write_stlb};

pub fn read_stl<R>(read: &mut R) -> Result<IndexedMesh>
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

    stl_reader.as_indexed_mesh(Some(metadata))
}

pub fn read_stla<R>(read: &mut R) -> Result<IndexedMesh>
where
    R: std::io::Read + std::io::Seek,
{
    let metadata = AsciiStlReader::extract_metadata(read)?;
    AsciiStlReader::create_triangles_iterator(read)?.as_indexed_mesh(Some(metadata))
}

pub fn read_stlb<R>(read: &mut R) -> Result<IndexedMesh>
where
    R: std::io::Read + std::io::Seek,
{
    BinaryStlReader::create_triangles_iterator(read)?.as_indexed_mesh(None)
}
