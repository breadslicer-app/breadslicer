/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::ffi::CStr;
use std::io::{BufReader, Read, Seek};

use anyhow::{Result, anyhow};
use sc_mesh_core::{MeshMetadata, Normal, Triangle, TriangleIterator, Vertex};

pub struct BinaryStlReader<'a> {
    reader: Box<dyn Read + 'a>,
    index: usize,
    size: usize,
}

impl<'a> Iterator for BinaryStlReader<'a> {
    type Item = Result<Triangle>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.size {
            self.index += 1;
            return Some(self.next_face());
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.size - self.index, Some(self.size - self.index))
    }
}

impl<'a> BinaryStlReader<'a> {
    pub fn extract_metadata<F: Read + Seek>(read: &mut F) -> Result<MeshMetadata> {
        let mut reader = Box::new(BufReader::new(&mut *read));
        let mut header = [0u8; 81]; // 1 byte extra for null-termination

        reader.read_exact(&mut header)?;
        read.seek(std::io::SeekFrom::Start(0))?;

        header[80] = 0u8; // To ensure we have a null-terminated string
        let mesh_name_cstr = CStr::from_bytes_until_nul(&header)?;
        let mesh_name = mesh_name_cstr.to_str()?.trim();

        if !mesh_name.is_ascii() {
            return Err(anyhow!("Mesh name is not ASCII"));
        }
        if mesh_name.is_empty() {
            return Ok(MeshMetadata { name: None });
        }

        Ok(MeshMetadata {
            name: Some(mesh_name.into()),
        })
    }

    pub fn create_triangles_iterator(
        read: &'a mut dyn Read,
    ) -> Result<Box<dyn TriangleIterator<Item = Result<Triangle>> + 'a>> {
        let mut reader = Box::new(BufReader::new(read));
        let mut header = [0u8; 80];
        reader.read_exact(&mut header)?;
        let mut count_buf = [0u8; 4];
        reader.read_exact(&mut count_buf)?;
        let num_faces = u32::from_le_bytes(count_buf) as usize;
        Ok(Box::new(BinaryStlReader {
            reader,
            index: 0,
            size: num_faces,
        })
            as Box<dyn TriangleIterator<Item = Result<Triangle>>>)
    }

    fn next_face(&mut self) -> Result<Triangle> {
        // Each triangle: 12 floats (normal + 3 vertices) * 4 bytes + 2 bytes attribute = 50 bytes
        let mut buf = [0u8; 50];
        self.reader.read_exact(&mut buf)?;

        let f = |offset: usize| -> f32 {
            f32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ])
        };

        let normal = Normal::new([f(0), f(4), f(8)]);
        let vertices = [
            Vertex::new([f(12), f(16), f(20)]),
            Vertex::new([f(24), f(28), f(32)]),
            Vertex::new([f(36), f(40), f(44)]),
        ];
        // buf[48..50] is the attribute byte count, ignored. TODO: check

        Ok(Triangle { normal, vertices })
    }
}

impl<'a> TriangleIterator for BinaryStlReader<'a> {}
