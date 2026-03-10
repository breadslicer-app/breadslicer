/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

mod stla;
mod stlb;

use anyhow::Result;

use stla::AsciiStlReader;
use stlb::BinaryStlReader;

use crate::core::{IndexedMesh, Triangle, TriangleIterator};

pub fn read_stl<R>(read: &mut R) -> Result<IndexedMesh>
where
    R: std::io::Read + std::io::Seek,
{
    create_stl_reader(read)?.as_indexed_mesh()
}

pub fn read_stla<R>(read: &mut R) -> Result<IndexedMesh>
where
    R: std::io::Read + std::io::Seek,
{
    AsciiStlReader::create_triangles_iterator(read)?.as_indexed_mesh()
}

pub fn read_stlb<R>(read: &mut R) -> Result<IndexedMesh>
where
    R: std::io::Read + std::io::Seek,
{
    BinaryStlReader::create_triangles_iterator(read)?.as_indexed_mesh()
}

pub fn create_stl_reader<'a, R>(
    read: &'a mut R,
) -> Result<Box<dyn TriangleIterator<Item = Result<Triangle>> + 'a>>
where
    R: std::io::Read + std::io::Seek,
{
    if AsciiStlReader::probe(read)? {
        AsciiStlReader::create_triangles_iterator(read)
    } else {
        BinaryStlReader::create_triangles_iterator(read)
    }
}
