/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod stla;
mod stlb;
mod writers;

use sc_mesh_core::SceneGraph;
use sc_mesh_core::TriangleIterator;
use sc_mesh_core::scene::Scene;

use crate::error::LoadStlError;
use stla::AsciiStlReader;
use stlb::BinaryStlReader;

pub use writers::{write_stla, write_stlb};

type Result<T> = std::result::Result<T, LoadStlError>;

pub fn read_stl<R>(read: &mut R) -> Result<SceneGraph>
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
    Ok(SceneGraph::try_from(Scene::from(mesh))?)
}

pub fn read_stla<R>(read: &mut R) -> Result<SceneGraph>
where
    R: std::io::Read + std::io::Seek,
{
    let metadata = AsciiStlReader::extract_metadata(read)?;
    let mesh = AsciiStlReader::create_triangles_iterator(read)?.as_indexed_mesh(Some(metadata))?;
    Ok(SceneGraph::try_from(Scene::from(mesh))?)
}

pub fn read_stlb<R>(read: &mut R) -> Result<SceneGraph>
where
    R: std::io::Read + std::io::Seek,
{
    let metadata = BinaryStlReader::extract_metadata(read)?;
    let mesh = BinaryStlReader::create_triangles_iterator(read)?.as_indexed_mesh(Some(metadata))?;
    Ok(SceneGraph::try_from(Scene::from(mesh))?)
}
