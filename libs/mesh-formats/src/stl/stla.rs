/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! ASCII STL reader.
//!
//! Keyword matching is case-insensitive. The original 3D Systems
//! specification uses lowercase keywords (`solid`, `facet`, `vertex`,
//! ...), but at least one widely distributed STL utility, admesh,
//! has emitted files with uppercase keywords. Slic3r adopted
//! case-insensitive parsing after a real-world crash report against
//! such a file: https://github.com/slic3r/Slic3r/issues/2971. This
//! reader follows the same convention: tokens are lowered at
//! line-split time and the probe regex matches case-insensitively
//! via the `(?i)` inline flag.

use std::io::{BufRead, BufReader, Read, Seek};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{MeshMetadata, Normal, Triangle, Vertex};

use crate::stl::TriangleIterator;

static ASTL_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)$").unwrap()
});
static ASTL_HEADER_RE_LN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)\n$").unwrap()
});

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

        match maybe_read_error {
            Ok(_) => Ok(ASTL_HEADER_RE_LN.is_match(&header)),
            Err(_) => Ok(false),
        }
    }

    pub fn extract_metadata<F: Read + Seek>(read: &mut F) -> Result<MeshMetadata> {
        let mut header = String::new();
        let maybe_read_error = BufReader::new(&mut *read).read_line(&mut header);

        // Try to seek back to start before evaluating potential read errors.
        read.seek(std::io::SeekFrom::Start(0))?;
        maybe_read_error?; // Unwraps any potential error

        match ASTL_HEADER_RE_LN.captures(&header) {
            Some(c) => Ok(MeshMetadata {
                name: match c.name("name") {
                    Some(n) => {
                        if !n.is_empty() {
                            Some(String::from(n.as_str()))
                        } else {
                            None
                        }
                    }
                    None => None,
                },
                ..Default::default()
            }),
            None => Ok(MeshMetadata::default()),
        }
    }

    pub fn create_triangles_iterator(
        read: &'a mut dyn Read,
    ) -> Result<Box<dyn TriangleIterator<Item = Result<Triangle>, Error = anyhow::Error> + 'a>>
    {
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

        // Lowercase every token. Keyword comparisons downstream
        // (`face_header[0] == "facet"`, `expect_static(...)`, etc.)
        // are written against bare lowercase literals; lowering at
        // this single point keeps them so. f32 parsing is unaffected:
        // `1.0E+02` and `1.0e+02` both parse to the same value.
        let lines = lines
            .map(|line_result| {
                line_result
                    .map(|l| {
                        l.split_whitespace()
                            .map(|t| t.to_ascii_lowercase())
                            .collect::<Vec<_>>()
                    })
                    .map_err(|err| err.into())
            })
            .filter(|result| !matches!(result, Ok(tokens) if tokens.is_empty()));

        Ok(Box::new(AsciiStlReader {
            lines: Box::new(lines),
        })
            as Box<
                dyn TriangleIterator<Item = Result<Triangle>, Error = anyhow::Error>,
            >)
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

        let mut normal_components = [0.0_f32; 3];
        AsciiStlReader::tokens_to_f32(&face_header[2..5], &mut normal_components)?;
        let mut normal = Normal::new(normal_components);

        self.expect_static(&["outer", "loop"])?;

        let mut vertices = [Vertex::new([0.0; 3]); 3];
        for vertex_slot in &mut vertices {
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
                    format!("Expected vertex f32 f32 f32, got {line:?}"),
                )
                .into());
            }
            let mut coords = [0.0_f32; 3];
            AsciiStlReader::tokens_to_f32(&line[1..4], &mut coords)?;
            *vertex_slot = Vertex::new(coords);
        }

        self.expect_static(&["endloop"])?;
        self.expect_static(&["endfacet"])?;

        // We interpret a zero-length normal vector as a hint to compute it at load time.
        // If the triangle is truly degenerate the recomputation can't produce a meaningful
        // direction; leave the zero normal in place so `validate_normal_vectors` flags it
        // as `ZeroAreaTriangle` rather than the parser bailing on the whole file.
        if normal == Normal::new([0.0_f32, 0.0_f32, 0.0_f32])
            && let Ok(recomputed) = compute_normal_from_triangle_vertices(&vertices)
        {
            normal = recomputed;
        }

        Ok(Some(Triangle::new(normal, vertices)))
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

impl<'a> TriangleIterator for AsciiStlReader<'a> {
    type Error = anyhow::Error;
}

#[cfg(test)]
mod test_ascii_stl_reader {
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

    use super::AsciiStlReader;

    static STL_1: &[u8; 304] = b"solid foobar
   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid foobar";

    static STL_2: &[u8; 291] = b"solid \n   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid";

    #[test]
    fn test_probe() {
        let mut reader_1 = std::io::Cursor::new(*STL_1);
        assert!(AsciiStlReader::probe(&mut reader_1).unwrap());

        let mut reader_2 = std::io::Cursor::new(*STL_2);
        assert!(AsciiStlReader::probe(&mut reader_2).unwrap());

        let mut reader_3 = std::io::Cursor::new(
            b"something
        else",
        );
        assert!(!AsciiStlReader::probe(&mut reader_3).unwrap());
    }

    #[test]
    fn test_extract_metadata() {
        let mut reader_1 = std::io::Cursor::new(*STL_1);
        let metadata_1 = AsciiStlReader::extract_metadata(&mut reader_1).unwrap();
        assert_eq!(
            MeshMetadata {
                name: Some("foobar".into()),
                ..Default::default()
            },
            metadata_1
        );

        let mut reader_2 = std::io::Cursor::new(*STL_2);
        let metadata_2 = AsciiStlReader::extract_metadata(&mut reader_2).unwrap();
        assert_eq!(MeshMetadata::default(), metadata_2);
    }

    #[test]
    fn test_as_indexed_mesh() {
        let mut reader_1 = std::io::Cursor::new(*STL_1);
        let mesh = AsciiStlReader::create_triangles_iterator(&mut reader_1)
            .unwrap()
            .as_indexed_mesh(None)
            .unwrap();

        assert_eq!(
            IndexedMesh {
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
            },
            mesh
        );
    }

    /// Same geometry as STL_1, every keyword uppercased. Mirrors the
    /// admesh-style files that crashed Slic3r before it adopted
    /// case-insensitive parsing.
    static STL_1_UPPER: &[u8] = b"SOLID foobar
   FACET NORMAL -1.000000e+000 0.000000e+000 0.000000e+000
      OUTER LOOP
         VERTEX 0.000000e+000 1.000000e+002 1.000000e+002
         VERTEX 0.000000e+000 1.000000e+002 0.000000e+000
         VERTEX 0.000000e+000 0.000000e+000 1.000000e+002
      ENDLOOP
   ENDFACET
ENDSOLID foobar";

    /// Same geometry as STL_1, keywords in title case. Catches the
    /// in-between of "all lower" and "all upper", in case any
    /// downstream comparison still uses an exact-match pattern.
    static STL_1_MIXED: &[u8] = b"Solid foobar
   Facet Normal -1.000000e+000 0.000000e+000 0.000000e+000
      Outer Loop
         Vertex 0.000000e+000 1.000000e+002 1.000000e+002
         Vertex 0.000000e+000 1.000000e+002 0.000000e+000
         Vertex 0.000000e+000 0.000000e+000 1.000000e+002
      EndLoop
   EndFacet
EndSolid foobar";

    fn expected_mesh() -> IndexedMesh {
        IndexedMesh {
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
        }
    }

    #[test]
    fn probe_accepts_uppercase_solid_header() {
        let mut reader = std::io::Cursor::new(STL_1_UPPER);
        assert!(AsciiStlReader::probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_accepts_mixed_case_solid_header() {
        let mut reader = std::io::Cursor::new(STL_1_MIXED);
        assert!(AsciiStlReader::probe(&mut reader).unwrap());
    }

    #[test]
    fn parses_all_uppercase_keywords() {
        let mut reader = std::io::Cursor::new(STL_1_UPPER);
        let mesh = AsciiStlReader::create_triangles_iterator(&mut reader)
            .unwrap()
            .as_indexed_mesh(None)
            .unwrap();
        assert_eq!(mesh, expected_mesh());
    }

    #[test]
    fn parses_mixed_case_keywords() {
        let mut reader = std::io::Cursor::new(STL_1_MIXED);
        let mesh = AsciiStlReader::create_triangles_iterator(&mut reader)
            .unwrap()
            .as_indexed_mesh(None)
            .unwrap();
        assert_eq!(mesh, expected_mesh());
    }

    #[test]
    fn extract_metadata_preserves_original_case_of_name() {
        // The keyword `SOLID` matches case-insensitively but the
        // captured name group retains its original case (regex
        // captures are verbatim slices of the input). A user who
        // exports an STL with a name like "PartA" must see it as
        // "PartA", not "parta".
        let bytes: &[u8] = b"SOLID PartA-001\nendsolid PartA-001";
        let mut reader = std::io::Cursor::new(bytes);
        let metadata = AsciiStlReader::extract_metadata(&mut reader).unwrap();
        assert_eq!(metadata.name.as_deref(), Some("PartA-001"));
    }
}
