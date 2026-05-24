/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::ffi::CStr;
use std::io::{BufReader, ErrorKind, Read, Seek};

use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{MeshMetadata, Normal, Triangle, TriangleIterator, Vertex};

use crate::error::LoadStlError;

type Result<T> = std::result::Result<T, LoadStlError>;

/// Hard ceiling on the declared triangle count in a binary STL
/// header. Files claiming more than this are refused at construction
/// time, before any payload bytes are read.
///
/// 200M triangles is ~10 GB of payload (50 bytes per triangle), well
/// above any legitimate engineering or scan mesh. The cap is a backstop
/// against a malicious or corrupt header for consumers that operate
/// without an earlier file-size gate (CLI, direct API). Most callers
/// pass [`DEFAULT_MAX_MESH_FILE_SIZE_BYTES`](crate::DEFAULT_MAX_MESH_FILE_SIZE_BYTES)
/// (2 GiB) to `load_mesh_from_path`, which gates upstream of this
/// parser cap; the cap only fires when that gate is absent or set
/// higher.
const MAX_BINARY_STL_TRIANGLES: usize = 200_000_000;

fn read_exact_section<R: Read>(reader: &mut R, buf: &mut [u8], what: &'static str) -> Result<()> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Err(LoadStlError::Truncated { what }),
        Err(e) => Err(LoadStlError::Io(e)),
    }
}

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

        read_exact_section(&mut reader, &mut header, "the 80-byte STL header")?;
        read.seek(std::io::SeekFrom::Start(0))?;

        header[80] = 0u8; // To ensure we have a null-terminated string
        let mesh_name_cstr = CStr::from_bytes_until_nul(&header)
            .map_err(|_| LoadStlError::HeaderNotNulTerminated)?;
        let mesh_name = mesh_name_cstr
            .to_str()
            .map_err(LoadStlError::HeaderNotUtf8)?
            .trim();

        if !mesh_name.is_ascii() {
            return Err(LoadStlError::HeaderNotAscii);
        }
        if mesh_name.is_empty() {
            return Ok(MeshMetadata::default());
        }

        Ok(MeshMetadata {
            name: Some(mesh_name.into()),
            ..Default::default()
        })
    }

    pub fn create_triangles_iterator(
        read: &'a mut dyn Read,
    ) -> Result<Box<dyn TriangleIterator<Item = Result<Triangle>, Error = LoadStlError> + 'a>> {
        let mut reader = Box::new(BufReader::new(read));
        let mut header = [0u8; 80];
        read_exact_section(&mut reader, &mut header, "the 80-byte STL header")?;
        let mut count_buf = [0u8; 4];
        read_exact_section(&mut reader, &mut count_buf, "the triangle count")?;
        let num_faces = u32::from_le_bytes(count_buf) as usize;
        if num_faces > MAX_BINARY_STL_TRIANGLES {
            return Err(LoadStlError::DeclaredTriangleCountExceedsCap {
                declared: num_faces,
                cap: MAX_BINARY_STL_TRIANGLES,
            });
        }
        Ok(Box::new(BinaryStlReader {
            reader,
            index: 0,
            size: num_faces,
        })
            as Box<
                dyn TriangleIterator<Item = Result<Triangle>, Error = LoadStlError>,
            >)
    }

    fn next_face(&mut self) -> Result<Triangle> {
        // Each triangle: 12 floats (normal + 3 vertices) * 4 bytes + 2 bytes attribute = 50 bytes
        let mut buf = [0u8; 50];
        read_exact_section(&mut self.reader, &mut buf, "a triangle record")?;

        let f = |offset: usize, what: &'static str| -> Result<f32> {
            let value = f32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]);
            if !value.is_finite() {
                return Err(LoadStlError::NonFiniteFloat { what, value });
            }
            Ok(value)
        };

        let mut normal = Normal::new([f(0, "normal.x")?, f(4, "normal.y")?, f(8, "normal.z")?]);
        let vertices = [
            Vertex::new([f(12, "v0.x")?, f(16, "v0.y")?, f(20, "v0.z")?]),
            Vertex::new([f(24, "v1.x")?, f(28, "v1.y")?, f(32, "v1.z")?]),
            Vertex::new([f(36, "v2.x")?, f(40, "v2.y")?, f(44, "v2.z")?]),
        ];
        // buf[48..50] is the per-triangle attribute byte count.
        // Currently discarded. The standard prescribes a zero value
        // but two real conventions store colour data here:
        //   VisCAM / SolidView: 15-bit RGB packed as R (bits 10-14),
        //     G (5-9), B (0-4) plus a validity bit (bit 15).
        //   Materialise Magics: per-facet colour in addition to a
        //     `COLOR=RGBA` field in the header, with R/G/B order
        //     reversed relative to the VisCAM convention.
        // When multi-material resin printing reaches the roadmap, a
        // `LoadStlWarning::AttributeBytesIgnored { face_idx, value }`
        // warning emitted here would let downstream surface it; that
        // requires a `LoadStlWarning` enum first (mirroring
        // `LoadObjWarning`).

        // We interpret a zero-length normal vector as a hint to compute it at load time.
        // If the triangle is truly degenerate the recomputation can't produce a meaningful
        // direction; leave the zero normal in place so `validate_normal_vectors` flags it
        // as `ZeroAreaTriangle` rather than the parser bailing on the whole file.
        if normal == Normal::new([0.0_f32, 0.0_f32, 0.0_f32])
            && let Ok(recomputed) = compute_normal_from_triangle_vertices(&vertices)
        {
            normal = recomputed;
        }

        Ok(Triangle::new(normal, vertices))
    }
}

impl<'a> TriangleIterator for BinaryStlReader<'a> {
    type Error = LoadStlError;
}

#[cfg(test)]
mod binary_stl_reader_tests {
    use std::io::Cursor;

    use super::{BinaryStlReader, MAX_BINARY_STL_TRIANGLES};

    /// Builds an 84-byte binary STL header: 80 zero-padded name bytes
    /// followed by a 4-byte little-endian declared triangle count. The
    /// payload is left empty so the reader sees an EOF the first time
    /// it tries to consume a triangle record.
    fn header_with_declared_count(count: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 84];
        buf[80..84].copy_from_slice(&count.to_le_bytes());
        buf
    }

    #[test]
    fn rejects_declared_count_just_above_cap() {
        let bytes = header_with_declared_count((MAX_BINARY_STL_TRIANGLES as u32) + 1);
        let mut cursor = Cursor::new(bytes);
        let err = BinaryStlReader::create_triangles_iterator(&mut cursor)
            .err()
            .expect("over-cap declared count must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds") && msg.contains("maximum"),
            "unexpected error message: {msg}",
        );
    }

    #[test]
    fn rejects_max_u32_declared_count() {
        // The pathological case from the audit. Without the cap this
        // produced billions of failed reads before propagating an
        // EOF; with the cap it errors at construction.
        let bytes = header_with_declared_count(u32::MAX);
        let mut cursor = Cursor::new(bytes);
        assert!(
            BinaryStlReader::create_triangles_iterator(&mut cursor).is_err(),
            "u32::MAX declared count must be rejected",
        );
    }

    #[test]
    fn accepts_declared_count_at_exact_cap() {
        // Boundary: equal to the cap must pass the check (the check
        // is `>`, not `>=`). The iterator is constructible; the body
        // is empty, so the first `next()` call surfaces a truncated
        // read, but construction itself succeeds.
        let bytes = header_with_declared_count(MAX_BINARY_STL_TRIANGLES as u32);
        let mut cursor = Cursor::new(bytes);
        assert!(
            BinaryStlReader::create_triangles_iterator(&mut cursor).is_ok(),
            "exact-cap declared count must construct successfully",
        );
    }

    #[test]
    fn accepts_zero_declared_count() {
        // A header that says "no triangles" must construct cleanly;
        // the resulting iterator yields nothing.
        let bytes = header_with_declared_count(0);
        let mut cursor = Cursor::new(bytes);
        let mut iter = BinaryStlReader::create_triangles_iterator(&mut cursor)
            .expect("zero-count header constructs");
        assert!(iter.next().is_none());
    }

    /// Builds a single-triangle binary STL whose attribute-byte-count
    /// (the trailing u16 in the triangle record) is non-zero. The
    /// rest of the record describes a unit XY triangle with normal
    /// +Z, so the parser has no excuse to fail.
    fn single_triangle_with_attribute_bytes(attr_bytes: u16) -> Vec<u8> {
        let mut bytes = header_with_declared_count(1);
        // Normal: (0, 0, 1).
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        // v0: (0, 0, 0).
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        // v1: (1, 0, 0).
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        // v2: (0, 1, 0).
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        // Attribute byte count.
        bytes.extend_from_slice(&attr_bytes.to_le_bytes());
        bytes
    }

    #[test]
    fn non_zero_attribute_bytes_are_silently_discarded() {
        // VisCAM / Magics colour conventions stash data in the
        // attribute bytes. This reader does not surface them today;
        // the test pins that behaviour so a future change adding a
        // warning channel updates this expectation deliberately.
        let bytes = single_triangle_with_attribute_bytes(0x8421);
        let mut cursor = Cursor::new(bytes);
        let mut iter =
            BinaryStlReader::create_triangles_iterator(&mut cursor).expect("header constructs");
        let tri = iter
            .next()
            .expect("one triangle")
            .expect("triangle parses cleanly");
        assert_eq!(tri.normal().coords(), [0.0, 0.0, 1.0]);
        let verts = tri.vertices();
        assert_eq!(verts[0].coords(), [0.0, 0.0, 0.0]);
        assert_eq!(verts[1].coords(), [1.0, 0.0, 0.0]);
        assert_eq!(verts[2].coords(), [0.0, 1.0, 0.0]);
        assert!(iter.next().is_none());
    }
}
