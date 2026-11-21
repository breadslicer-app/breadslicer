/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use rayon::iter::ParallelIterator;
use rayon::slice::ParallelSlice;
use sc_mesh_core::{MeshMetadata, Triangle, Vertex};
use sc_scene::Scene;

use crate::MAX_MESH_ELEMENT_COUNT;
use crate::error::LoadStlError;
use crate::messages::LoadStlWarning;
use crate::stl::{
    Facet, STL_HEADER_BYTES, STL_TRIANGLE_COUNT_BYTES, STL_TRIANGLE_RECORD_BYTES, kept_normal,
    mesh_from_facets,
};

type Result<T> = std::result::Result<T, LoadStlError>;

const PREAMBLE_BYTES: usize = STL_HEADER_BYTES + STL_TRIANGLE_COUNT_BYTES;

/// Reads a binary STL document from an in-memory byte slice into a 1-leaf
/// scene, plus any header warning.
pub(crate) fn read_stlb_slice(bytes: &[u8]) -> Result<(Scene, Vec<LoadStlWarning>)> {
    let (metadata, header_warning) = extract_metadata(bytes)?;
    let (facets, left_out) = parse_triangle_records(bytes)?;
    // A file of nothing but such triangles has nothing to open.
    if facets.is_empty() && left_out > 0 {
        return Err(LoadStlError::NonFiniteFloat {
            what: "every vertex",
            value: f32::NAN,
        });
    }
    let mesh = mesh_from_facets(metadata, facets)?;
    let mut warnings: Vec<LoadStlWarning> = header_warning.into_iter().collect();
    if left_out > 0 {
        warnings.push(LoadStlWarning::FacetsWithoutCoordinates { count: left_out });
    }
    Ok((Scene::from(mesh), warnings))
}

/// Reports whether the byte length equals the binary layout exactly for the
/// triangle count declared at offset 80: header, count field, and `count`
/// fixed-width records, nothing more. Real binary files whose header text
/// happens to start with `solid` exist, and this equality is what routes them
/// to the binary parser regardless of that text.
pub(crate) fn matches_exact_binary_layout(bytes: &[u8]) -> bool {
    let Some(count_bytes) = bytes.get(STL_HEADER_BYTES..PREAMBLE_BYTES) else {
        return false;
    };
    let Ok(count_bytes) = <[u8; STL_TRIANGLE_COUNT_BYTES]>::try_from(count_bytes) else {
        return false;
    };
    let declared = u32::from_le_bytes(count_bytes) as usize;
    declared
        .checked_mul(STL_TRIANGLE_RECORD_BYTES)
        .and_then(|records| records.checked_add(PREAMBLE_BYTES))
        .is_some_and(|total| total == bytes.len())
}

/// Outcome of decoding the 80-byte header's comment field.
enum HeaderText {
    /// Nothing before the first nul, or whitespace only.
    Empty,
    /// Printable text, kept as the mesh name.
    Name(String),
    /// Not usable text: invalid UTF-8 or control characters. Real files carry
    /// raw bytes here (colour conventions, codepage text, exporter leftovers);
    /// the geometry is unaffected.
    NotText,
}

/// Decodes the header's comment field: the bytes before the first nul (all 80
/// when no nul exists), as UTF-8. Text with control characters left after
/// trimming is treated the same as undecodable bytes: the header is a comment
/// with no mandated charset, so nothing there may fail a load or become a
/// garbled name.
fn decode_header_text(header: &[u8]) -> HeaderText {
    let prefix = match header.iter().position(|&byte| byte == 0) {
        Some(nul) => &header[..nul],
        None => header,
    };
    let Ok(text) = str::from_utf8(prefix) else {
        return HeaderText::NotText;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return HeaderText::Empty;
    }
    if trimmed.chars().any(char::is_control) {
        return HeaderText::NotText;
    }
    HeaderText::Name(trimmed.to_string())
}

/// Returns the mesh metadata of the 80-byte header. The header is a comment
/// field: when it holds printable text the trimmed text becomes the mesh name;
/// anything else (all-zero, whitespace-only, raw bytes) yields the default
/// nameless metadata, with a warning when non-text content was actually
/// present. Only a document too short to hold the header errors.
pub(crate) fn extract_metadata(bytes: &[u8]) -> Result<(MeshMetadata, Option<LoadStlWarning>)> {
    let Some(header) = bytes.get(..STL_HEADER_BYTES) else {
        return Err(LoadStlError::Truncated {
            what: "the 80-byte STL header",
        });
    };

    Ok(match decode_header_text(header) {
        HeaderText::Empty => (MeshMetadata::default(), None),
        HeaderText::Name(name) => (
            MeshMetadata {
                name: Some(name),
                ..Default::default()
            },
            None,
        ),
        HeaderText::NotText => (MeshMetadata::default(), Some(LoadStlWarning::HeaderNotText)),
    })
}

/// The triangles of the document, and how many it left out because a vertex
/// is not a number (NaN or infinite).
fn parse_triangle_records(bytes: &[u8]) -> Result<(Vec<Facet>, usize)> {
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
    if available < num_faces {
        return Err(LoadStlError::Truncated {
            what: "a triangle record",
        });
    }
    let records = &body[..num_faces * STL_TRIANGLE_RECORD_BYTES];

    let parsed: Vec<Option<Facet>> = if num_faces < PARALLEL_PARSE_MIN_RECORDS {
        records
            .as_chunks::<STL_TRIANGLE_RECORD_BYTES>()
            .0
            .iter()
            .map(|record| parse_triangle_record(record))
            .collect()
    } else {
        records
            .par_chunks(STL_TRIANGLE_RECORD_BYTES)
            .map(parse_triangle_record)
            .collect()
    };
    let left_out = parsed.iter().filter(|f| f.is_none()).count();
    Ok((parsed.into_iter().flatten().collect(), left_out))
}

/// Crossover between the sequential and the parallel record parse. Measured:
/// the parallel split starts to be faster between 4k and 8k records; below that
/// the fork-join overhead is about equal to the parsing work.
pub(super) const PARALLEL_PARSE_MIN_RECORDS: usize = 8192;

/// Parses a 50-byte triangle record: normal, 3 vertices, attribute bytes.
/// `None` when a vertex is not a number: such a triangle has no place in
/// space. A declared normal that is not a number gives way to the normal of
/// the vertices (`kept_normal`).
fn parse_triangle_record(record: &[u8]) -> Option<Facet> {
    let f = |offset: usize| -> f32 {
        f32::from_le_bytes([
            record[offset],
            record[offset + 1],
            record[offset + 2],
            record[offset + 3],
        ])
    };
    let fields = [
        f(0),
        f(4),
        f(8),
        f(12),
        f(16),
        f(20),
        f(24),
        f(28),
        f(32),
        f(36),
        f(40),
        f(44),
    ];
    if fields[3..].iter().any(|value| !value.is_finite()) {
        return None;
    }
    let declared = [fields[0], fields[1], fields[2]];
    let vertices = [
        Vertex::new([fields[3], fields[4], fields[5]]),
        Vertex::new([fields[6], fields[7], fields[8]]),
        Vertex::new([fields[9], fields[10], fields[11]]),
    ];
    // record[48..50] is the per-triangle attribute byte count, discarded. The
    // standard prescribes 0, but 2 real conventions store colour data here:
    //   VisCAM and SolidView: 15-bit RGB packed as R (bits 10-14), G (5-9),
    //     B (0-4) plus a validity bit (bit 15).
    //   Materialise Magics: per-facet colour in addition to a `COLOR=RGBA`
    //     field in the header, with the red, green and blue order reversed
    //     relative to the VisCAM convention.
    Some(Facet {
        declared_normal: kept_normal(declared, &vertices),
        triangle: Triangle::new(vertices),
    })
}

#[cfg(test)]
mod binary_stl_reader_tests {
    use super::{
        MAX_MESH_ELEMENT_COUNT, PARALLEL_PARSE_MIN_RECORDS, PREAMBLE_BYTES, STL_HEADER_BYTES,
        STL_TRIANGLE_RECORD_BYTES, read_stlb_slice,
    };
    use crate::error::LoadStlError;
    use crate::messages::LoadStlWarning;

    /// Builds an 84-byte binary STL preamble: an all-zero header followed by a
    /// little-endian declared triangle count, with no triangle records after
    /// it.
    fn header_with_declared_count(count: u32) -> Vec<u8> {
        let mut buf = vec![0u8; PREAMBLE_BYTES];
        buf[STL_HEADER_BYTES..PREAMBLE_BYTES].copy_from_slice(&count.to_le_bytes());
        buf
    }

    #[test]
    fn exact_layout_is_recognized() {
        use super::matches_exact_binary_layout;

        // 0 records: preamble alone.
        assert!(matches_exact_binary_layout(&header_with_declared_count(0)));

        // 2 records, exact length.
        let mut bytes = header_with_declared_count(2);
        bytes.extend(std::iter::repeat_n(0u8, 2 * STL_TRIANGLE_RECORD_BYTES));
        assert!(matches_exact_binary_layout(&bytes));

        // 1 byte short and 1 byte long both fail the equality.
        let mut short = bytes.clone();
        short.pop();
        assert!(!matches_exact_binary_layout(&short));
        let mut long = bytes;
        long.push(0);
        assert!(!matches_exact_binary_layout(&long));

        // Too short to even hold the count field.
        assert!(!matches_exact_binary_layout(&[0u8; STL_HEADER_BYTES]));

        // A declared count of `u32::MAX` never matches the 84-byte input. On
        // 32-bit targets the record length overflows `usize`, and the checked
        // arithmetic gives no match instead of a wrapped length.
        assert!(!matches_exact_binary_layout(&header_with_declared_count(
            u32::MAX
        )));
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
        // The largest declared count: the cap check rejects it before the
        // parser touches any record.
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
        // Boundary: a count equal to the cap passes the check (the check is
        // `>`, not `>=`). The body is empty, so the parse then reports a
        // truncated record, not the cap error.
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
        // A header that says "no triangles" must parse cleanly into an empty
        // 1-leaf scene.
        let bytes = header_with_declared_count(0);
        let (scene, warnings) = read_stlb_slice(&bytes).expect("zero-count document parses");
        assert!(warnings.is_empty());
        let leaf = scene.first_leaf_id().expect("one leaf");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert!(mesh.faces().is_empty());
        assert!(mesh.vertices().is_empty());
    }

    /// Builds a single-triangle binary STL whose attribute byte count (the
    /// trailing u16 of the triangle record) is `attr_bytes`. The rest of the
    /// record is a valid unit triangle in the XY plane that declares the normal
    /// +Z.
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
        // The VisCAM and Magics colour conventions store data in the attribute
        // bytes. The reader discards them without a warning; this test pins
        // that behaviour.
        let bytes = single_triangle_with_attribute_bytes(0x8421);
        let (scene, warnings) = read_stlb_slice(&bytes).expect("document parses cleanly");
        assert!(warnings.is_empty());
        let leaf = scene.first_leaf_id().expect("one leaf");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(
            mesh.declared_facet_normals(),
            Some(&[[0, 0, 0x3f80_0000]][..])
        );
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

    /// Builds a document declaring `declared` triangles and carrying `actual`
    /// copies of the same valid unit-triangle record.
    fn repeated_record_document(declared: u32, actual: usize) -> Vec<u8> {
        let mut bytes = header_with_declared_count(declared);
        let record = single_triangle_with_attribute_bytes(0)[PREAMBLE_BYTES..].to_vec();
        for _ in 0..actual {
            bytes.extend_from_slice(&record);
        }
        bytes
    }

    fn corrupt_float(bytes: &mut [u8], record_idx: usize, field_offset: usize) {
        let at = PREAMBLE_BYTES + record_idx * STL_TRIANGLE_RECORD_BYTES + field_offset;
        bytes[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    }

    /// Triangles whose vertices are not numbers are left out and counted, in
    /// both paths; the rest of the file opens (#677).
    #[test]
    fn triangles_without_coordinates_are_left_out_and_told() {
        for n in [3, PARALLEL_PARSE_MIN_RECORDS + 1000] {
            let mut bytes = repeated_record_document(n as u32, n);
            corrupt_float(&mut bytes, 1, 44); // v2.z
            corrupt_float(&mut bytes, n - 1, 12); // v0.x
            let (scene, warnings) = read_stlb_slice(&bytes).expect("the file opens");
            assert_eq!(
                warnings,
                [LoadStlWarning::FacetsWithoutCoordinates { count: 2 }]
            );
            let (soup, left_out) = super::parse_triangle_records(&bytes).expect("parses");
            assert_eq!((soup.len(), left_out), (n - 2, 2));
            drop(scene);
        }
    }

    #[test]
    fn a_truncated_document_is_still_an_error_with_a_bad_vertex() {
        let n = PARALLEL_PARSE_MIN_RECORDS + 1000;
        let mut bytes = repeated_record_document(n as u32, n - 1);
        corrupt_float(&mut bytes, 100, 28); // v1.y
        assert!(matches!(
            read_stlb_slice(&bytes),
            Err(LoadStlError::Truncated { .. }),
        ));
    }

    #[test]
    fn a_nan_normal_takes_the_normal_of_its_vertices() {
        // Some exporters write NaN normals: the file opens, in both paths, and
        // the facet keeps the normal its vertices give.
        for n in [1, PARALLEL_PARSE_MIN_RECORDS + 10] {
            let mut bytes = repeated_record_document(n as u32, n);
            for record in 0..n {
                for field in [0, 4, 8] {
                    corrupt_float(&mut bytes, record, field);
                }
            }
            let (soup, left_out) =
                super::parse_triangle_records(&bytes).expect("a NaN normal is no error");
            assert_eq!(left_out, 0);
            assert_eq!(soup.len(), n);
            let vertices = soup[0].triangle.vertices();
            let expected = super::kept_normal([f32::NAN; 3], &vertices);
            assert!(
                expected.iter().any(|&bits| bits != 0),
                "the record is not degenerate"
            );
            assert!(soup.iter().all(|f| f.declared_normal == expected));
            assert!(
                soup[0]
                    .declared_normal
                    .map(f32::from_bits)
                    .iter()
                    .all(|c| c.is_finite())
            );
        }
    }
}

#[cfg(test)]
mod header_decode_tests {
    use super::{HeaderText, STL_HEADER_BYTES, decode_header_text};

    fn padded(prefix: &[u8]) -> Vec<u8> {
        let mut header = vec![0u8; STL_HEADER_BYTES];
        header[..prefix.len()].copy_from_slice(prefix);
        header
    }

    fn as_name(header: &[u8]) -> Option<String> {
        match decode_header_text(header) {
            HeaderText::Name(name) => Some(name),
            HeaderText::Empty | HeaderText::NotText => None,
        }
    }

    #[test]
    fn all_zero_is_empty() {
        assert!(matches!(
            decode_header_text(&padded(b"")),
            HeaderText::Empty
        ));
    }

    #[test]
    fn whitespace_only_is_empty() {
        assert!(matches!(
            decode_header_text(&padded(b"  \t ")),
            HeaderText::Empty,
        ));
    }

    #[test]
    fn ascii_text_is_a_trimmed_name() {
        assert_eq!(
            as_name(&padded(b"  Part.001  ")).as_deref(),
            Some("Part.001")
        );
    }

    #[test]
    fn utf8_text_is_a_name() {
        assert_eq!(
            as_name(&padded("Küche".as_bytes())).as_deref(),
            Some("Küche")
        );
    }

    #[test]
    fn full_width_text_without_nul_is_a_name() {
        assert_eq!(as_name(&[b'a'; STL_HEADER_BYTES]), Some("a".repeat(80)));
    }

    #[test]
    fn text_stops_at_the_first_nul() {
        let mut header = padded(b"front");
        header[10..14].copy_from_slice(b"rear");
        assert_eq!(as_name(&header).as_deref(), Some("front"));
    }

    #[test]
    fn invalid_utf8_is_not_text() {
        assert!(matches!(
            decode_header_text(&padded(b"K\xFCche")),
            HeaderText::NotText,
        ));
    }

    #[test]
    fn control_characters_are_not_text() {
        assert!(matches!(
            decode_header_text(&padded(b"\x07\x01\x07\x01")),
            HeaderText::NotText,
        ));
    }

    #[test]
    fn interior_control_character_poisons_the_whole_header() {
        assert!(matches!(
            decode_header_text(&padded(b"part\x08one")),
            HeaderText::NotText,
        ));
    }
}
