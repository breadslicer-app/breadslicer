/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::io::{BufRead, BufReader, Read, Seek};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::core::{IndexedMesh, IndexedTriangle, Normal, Triangle, Vertex};

pub trait TriangleIterator: Iterator<Item = Result<Triangle>> {
    fn as_indexed_mesh(&mut self) -> Result<IndexedMesh> {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut vertex_to_index = std::collections::HashMap::new();

        // Do not reserve memory in those structures based on size_hint, because we might have just
        // read bogus data.
        let mut vertex_indices: [u32; 3] = [0; 3];
        for t in self {
            let t = t?;
            for (i, vertex) in t.vertices.iter().enumerate() {
                // f32 has no Eq and no Hash, but comparing the bits will do.
                // This has the effect that if any coordinate is NaN (which does not make sense
                // anyway), its NaN payload bits will be used as the identity of the vertex.
                let bitpattern = vertex.0.map(f32::to_bits);
                let index = *vertex_to_index
                    .entry(bitpattern)
                    .or_insert_with(|| vertices.len());

                if index == vertices.len() {
                    vertices.push(*vertex);
                }

                vertex_indices[i] = index as u32;
            }

            triangles.push(IndexedTriangle {
                normal: t.normal,
                vertices: vertex_indices,
            });
        }

        vertices.shrink_to_fit();
        triangles.shrink_to_fit();

        Ok(IndexedMesh {
            vertices,
            faces: triangles,
        })
    }
}

static ASTL_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"solid [a-zA-Z0-9_]*\n").unwrap());

pub struct AsciiStlReader<'a> {
    lines: Box<dyn Iterator<Item = Result<Vec<String>>> + 'a>,
}

impl<'a> Iterator for AsciiStlReader<'a> {
    type Item = Result<Triangle>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.next_face() {
            Ok(None) => None,
            Ok(Some(t)) => Some(Ok(t)),
            Err(e) => Some(Err(e)),
        }
    }
}

impl<'a> AsciiStlReader<'a> {
    pub fn probe<F: Read + Seek>(read: &mut F) -> Result<bool> {
        let mut header = String::new();
        let maybe_read_error = BufReader::new(&mut *read).read_line(&mut header);

        // Try to seek back to start before evaluating potential read errors.
        read.seek(std::io::SeekFrom::Start(0))?;
        maybe_read_error?; // Unwraps any potential error

        // static RE: LazyLock<Regex> =
        //     LazyLock::new(|| Regex::new(r"solid [a-zA-Z0-9_]*\n").unwrap());
        return Ok(ASTL_HEADER_RE.is_match(&header));
    }

    pub fn create_triangles_iterator(
        read: &'a mut dyn Read,
    ) -> Result<Box<dyn TriangleIterator<Item = Result<Triangle>> + 'a>> {
        let mut lines = BufReader::new(read).lines();

        // Consume first line
        match lines.next() {
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(ref line)) if !ASTL_HEADER_RE.is_match(line) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Ascii STL does not start with \"solid \"",
                )
                .into());
            }
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "File seems to be empty",
                )
                .into());
            }
            _ => {}
        }

        let lines = lines
            .map(|line_result| {
                line_result
                    .map(|l| {
                        l.split_whitespace()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                    })
                    .map_err(|err| err.into())
            })
            .filter(|result| !matches!(result, Ok(tokens) if tokens.is_empty()));

        Ok(Box::new(AsciiStlReader {
            lines: Box::new(lines),
        })
            as Box<dyn TriangleIterator<Item = Result<Triangle>>>)
    }

    fn next_face(&mut self) -> Result<Option<Triangle>> {
        let Some(face_header) = self.lines.next() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "EOF while expecting facet or endsolid.",
            )
            .into());
        };

        // Our iterator is fallible
        let face_header = face_header?;

        if !face_header.is_empty() && face_header[0] == "endsolid" {
            return Ok(None);
        }

        if face_header.len() != 5 || face_header[0] != "facet" || face_header[1] != "normal" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid facet header: {face_header:?}"),
            )
            .into());
        }

        let mut result_normal = Normal::default();

        AsciiStlReader::tokens_to_f32(&face_header[2..5], &mut result_normal.0[0..3])?;

        self.expect_static(&["outer", "loop"])?;
        let mut result_vertices = [Vertex::default(); 3];
        for vertex_result in &mut result_vertices {
            let Some(line) = self.lines.next() else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF while expecting vertex",
                )
                .into());
            };

            let line = line?;

            if line.len() != 4 || line[0] != "vertex" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("vertex f32 f32 f32, got {line:?}"),
                )
                .into());
            }
            AsciiStlReader::tokens_to_f32(&line[1..4], &mut vertex_result.0[0..3])?;
        }

        self.expect_static(&["endloop"])?;
        self.expect_static(&["endfacet"])?;

        Ok(Some(Triangle {
            normal: result_normal,
            vertices: result_vertices,
        }))
    }

    fn tokens_to_f32(tokens: &[String], output: &mut [f32]) -> Result<()> {
        assert_eq!(tokens.len(), output.len());

        for (token, out) in tokens.iter().zip(output.iter_mut()) {
            let f = token
                .parse::<f32>()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

            if !f.is_finite() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("expected finite f32, got {f} which is {:?}", f.classify()),
                )
                .into());
            }
            *out = f;
        }

        Ok(())
    }

    fn expect_static(&mut self, expectation: &[&str]) -> Result<()> {
        let Some(line) = self.lines.next() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("EOF while expecting {expectation:?}"),
            )
            .into());
        };

        let line = line?;

        if line != expectation {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected {expectation:?}, got {line:?}"),
            )
            .into());
        }

        Ok(())
    }
}

impl<'a> TriangleIterator for AsciiStlReader<'a> {}

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
    todo!("Not yet implemented")
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
        todo!("Not implemented yet")
        // BinaryStlReader::create_triangle_iterator(read)
    }
}

#[cfg(test)]
mod test {
    use super::AsciiStlReader;

    static STL_1: &'static [u8; 304] = b"solid foobar
   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid foobar";

    static STL_2: &'static [u8; 291] = b"solid 
   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid";

    #[test]
    fn ascii_stl_reader_probe() {
        let mut reader_1 = std::io::Cursor::new(STL_1.clone());
        assert!(AsciiStlReader::probe(&mut reader_1).unwrap());

        let mut reader_2 = std::io::Cursor::new(STL_2.clone());
        assert!(AsciiStlReader::probe(&mut reader_2).unwrap());

        let mut reader_3 = std::io::Cursor::new(
            b"something
        else",
        );
        assert!(!AsciiStlReader::probe(&mut reader_3).unwrap());
    }
}
