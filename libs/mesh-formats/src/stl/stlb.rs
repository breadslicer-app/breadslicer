/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::ffi::CStr;

use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{MeshMetadata, Normal, Scene, Triangle, Vertex, indexed_mesh_from_soup};

use crate::MAX_MESH_ELEMENT_COUNT;
use crate::error::LoadStlError;
use crate::stl::{STL_HEADER_BYTES, STL_TRIANGLE_COUNT_BYTES, STL_TRIANGLE_RECORD_BYTES};

type Result<T> = std::result::Result<T, LoadStlError>;

const PREAMBLE_BYTES: usize = STL_HEADER_BYTES + STL_TRIANGLE_COUNT_BYTES;

/// Reads a binary STL document from an in-memory byte slice into a
/// one-leaf scene.
pub(crate) fn read_stlb_slice(bytes: &[u8]) -> Result<Scene> {
    let metadata = extract_metadata(bytes)?;
    let soup = parse_triangle_records(bytes)?;
    let mesh = indexed_mesh_from_soup(Some(metadata), soup, u32::MAX as usize)?;
    Ok(Scene::from(mesh))
}

/// Mesh name from the 80-byte header: the bytes up to the first nul,
/// trimmed, required to be ASCII. An all-zero or whitespace-only
/// header yields the default (nameless) metadata.
pub(crate) fn extract_metadata(bytes: &[u8]) -> Result<MeshMetadata> {
    let Some(header) = bytes.get(..STL_HEADER_BYTES) else {
        return Err(LoadStlError::Truncated {
            what: "the 80-byte STL header",
        });
    };

    // 1 byte extra guarantees nul termination for the CStr parse.
    let mut buf = [0u8; STL_HEADER_BYTES + 1];
    buf[..STL_HEADER_BYTES].copy_from_slice(header);
    let mesh_name_cstr =
        CStr::from_bytes_until_nul(&buf).map_err(|_| LoadStlError::HeaderNotNulTerminated)?;
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

/// Declared triangle count (bounds-checked against the cap) plus the
/// triangle records that follow, parsed in declaration order.
///
/// Truncation is reported only after the records that ARE present have
/// parsed cleanly, so a file that is both truncated and malformed
/// reports the first error in file order, exactly like the retired
/// record-by-record streaming reader.
fn parse_triangle_records(bytes: &[u8]) -> Result<Vec<Triangle>> {
    let Some(count_bytes) = bytes.get(STL_HEADER_BYTES..PREAMBLE_BYTES) else {
        return Err(LoadStlError::Truncated {
            what: "the triangle count",
        });
    };
    let num_faces = u32::from_le_bytes([
        count_bytes[0],
        count_bytes[1],
        count_bytes[2],
        count_bytes[3],
    ]) as usize;
    if num_faces > MAX_MESH_ELEMENT_COUNT {
        return Err(LoadStlError::DeclaredTriangleCountExceedsCap {
            declared: num_faces,
            cap: MAX_MESH_ELEMENT_COUNT,
        });
    }

    let body = &bytes[PREAMBLE_BYTES..];
    let available = body.len() / STL_TRIANGLE_RECORD_BYTES;
    let mut soup: Vec<Triangle> = Vec::with_capacity(num_faces.min(available));
    for record in body.chunks_exact(STL_TRIANGLE_RECORD_BYTES).take(num_faces) {
        soup.push(parse_triangle_record(record)?);
    }
    if soup.len() < num_faces {
        return Err(LoadStlError::Truncated {
            what: "a triangle record",
        });
    }
    Ok(soup)
}

/// One 50-byte triangle record: normal, 3 vertices, attribute bytes.
fn parse_triangle_record(record: &[u8]) -> Result<Triangle> {
    let f = |offset: usize, what: &'static str| -> Result<f32> {
        let value = f32::from_le_bytes([
            record[offset],
            record[offset + 1],
            record[offset + 2],
            record[offset + 3],
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
    // record[48..50] is the per-triangle attribute byte count.
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

#[cfg(test)]
mod binary_stl_reader_tests {
    use super::{MAX_MESH_ELEMENT_COUNT, PREAMBLE_BYTES, STL_HEADER_BYTES, read_stlb_slice};
    use crate::error::LoadStlError;

    /// Builds an 84-byte binary STL preamble: a zero-padded name field
    /// followed by a little-endian declared triangle count. The payload
    /// is left empty so the parser runs out of bytes the first time it
    /// looks for a triangle record.
    fn header_with_declared_count(count: u32) -> Vec<u8> {
        let mut buf = vec![0u8; PREAMBLE_BYTES];
        buf[STL_HEADER_BYTES..PREAMBLE_BYTES].copy_from_slice(&count.to_le_bytes());
        buf
    }

    #[test]
    fn rejects_declared_count_just_above_cap() {
        let bytes = header_with_declared_count((MAX_MESH_ELEMENT_COUNT as u32) + 1);
        let err = read_stlb_slice(&bytes).expect_err("over-cap declared count must be rejected");
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
        // EOF; with the cap it errors before touching any record.
        let bytes = header_with_declared_count(u32::MAX);
        assert!(
            matches!(
                read_stlb_slice(&bytes),
                Err(LoadStlError::DeclaredTriangleCountExceedsCap { .. }),
            ),
            "u32::MAX declared count must be rejected",
        );
    }

    #[test]
    fn accepts_declared_count_at_exact_cap() {
        // Boundary: equal to the cap must pass the check (the check
        // is `>`, not `>=`). The body is empty, so the parse then
        // reports a truncated record, NOT the cap error.
        let bytes = header_with_declared_count(MAX_MESH_ELEMENT_COUNT as u32);
        assert!(
            matches!(
                read_stlb_slice(&bytes),
                Err(LoadStlError::Truncated {
                    what: "a triangle record",
                }),
            ),
            "exact-cap declared count must pass the cap check",
        );
    }

    #[test]
    fn accepts_zero_declared_count() {
        // A header that says "no triangles" must parse cleanly into
        // an empty one-leaf scene.
        let bytes = header_with_declared_count(0);
        let scene = read_stlb_slice(&bytes).expect("zero-count document parses");
        let leaf = scene.first_leaf_id().expect("one leaf");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert!(mesh.faces().is_empty());
        assert!(mesh.vertices().is_empty());
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
        let scene = read_stlb_slice(&bytes).expect("document parses cleanly");
        let leaf = scene.first_leaf_id().expect("one leaf");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].normal.coords(), [0.0, 0.0, 1.0]);
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.vertices()[0].coords(), [0.0, 0.0, 0.0]);
        assert_eq!(mesh.vertices()[1].coords(), [1.0, 0.0, 0.0]);
        assert_eq!(mesh.vertices()[2].coords(), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn truncated_mid_header_reports_the_header() {
        let bytes = vec![0u8; 40];
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::Truncated {
                what: "the 80-byte STL header",
            }),
        ));
    }

    #[test]
    fn truncated_mid_count_reports_the_count() {
        let bytes = vec![0u8; STL_HEADER_BYTES + 2];
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::Truncated {
                what: "the triangle count",
            }),
        ));
    }

    #[test]
    fn truncated_mid_record_reports_a_record() {
        let mut bytes = single_triangle_with_attribute_bytes(0);
        bytes.truncate(PREAMBLE_BYTES + 30);
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::Truncated {
                what: "a triangle record",
            }),
        ));
    }

    #[test]
    fn missing_second_record_reports_a_record() {
        let mut bytes = single_triangle_with_attribute_bytes(0);
        bytes[STL_HEADER_BYTES..PREAMBLE_BYTES].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::Truncated {
                what: "a triangle record",
            }),
        ));
    }

    #[test]
    fn earlier_bad_float_wins_over_later_truncation() {
        // Record 0 carries a NaN normal.x AND the file is missing its
        // second declared record. The record error comes first in
        // file order, matching the retired streaming reader.
        let mut bytes = single_triangle_with_attribute_bytes(0);
        bytes[STL_HEADER_BYTES..PREAMBLE_BYTES].copy_from_slice(&2u32.to_le_bytes());
        bytes[PREAMBLE_BYTES..PREAMBLE_BYTES + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::NonFiniteFloat {
                what: "normal.x",
                ..
            }),
        ));
    }
}
