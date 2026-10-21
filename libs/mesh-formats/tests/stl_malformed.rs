/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Malformed-input integration tests for the STL readers. They pin the error
 * branches of `stla.rs` and `stlb.rs` with fixture buffers, so that a refactor
 * cannot replace a hard error with a default value (or the opposite) without a
 * test failure.
 *
 * `stl_round_trips.rs` covers the happy paths.
 */

use std::io::Cursor;

use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::stl::{read_stla, read_stlb};
use sc_scene::Scene;

fn unwrap_single_mesh(graph: &Scene) -> &IndexedMesh {
    assert_eq!(
        graph.reachable_leaf_count(),
        1,
        "expected exactly one reachable leaf"
    );
    let id = graph
        .first_leaf_id()
        .expect("reachable_leaf_count == 1 implies a leaf exists");
    graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
}

/// Returns the facet normals that the file of `mesh` declared, as values.
fn declared_normals(mesh: &IndexedMesh) -> Vec<[f32; 3]> {
    mesh.declared_facet_normals()
        .expect("an STL reader keeps the declared facet normals")
        .iter()
        .map(|bits| bits.map(f32::from_bits))
        .collect()
}

#[cfg(test)]
mod ascii_stl_malformed_tests {
    use super::*;

    #[test]
    fn empty_input_is_an_error() {
        let mut reader = Cursor::new(Vec::<u8>::new());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn missing_solid_header_is_an_error() {
        // No leading "solid ": `parse_solid_header` rejects the first line.
        let buf = b"facet normal 0 0 0\nendsolid\n";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn truncated_after_solid_is_an_error() {
        // Header is fine, but the file ends before any facet appears and
        // without an `endsolid` either.
        let buf = b"solid foo\n";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn invalid_facet_header_is_an_error() {
        // `facet NOTNORMAL 0 0 0`: the second token has to be the literal
        // `normal`.
        let buf = b"\
solid foo
  facet NOTNORMAL 0 0 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn non_numeric_normal_component_is_an_error() {
        let buf = b"\
solid foo
  facet normal NaN 0 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        // "NaN" parses as a float but is rejected by the finite-only check in
        // `parse_finite_f32`.
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn unparseable_vertex_coordinate_is_an_error() {
        let buf = b"\
solid foo
  facet normal 0 0 1
    outer loop
      vertex zero 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn missing_outer_loop_is_an_error() {
        // After `facet normal ...` the next line must be `outer loop`.
        let buf = b"\
solid foo
  facet normal 0 0 1
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn vertex_line_with_wrong_token_count_is_an_error() {
        // Only 2 coordinates supplied for the first vertex.
        let buf = b"\
solid foo
  facet normal 0 0 1
    outer loop
      vertex 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn truncated_mid_facet_is_an_error() {
        // The facet starts but the file ends before all 3 vertices and the
        // closing `endloop` and `endfacet` lines are read.
        let buf = b"\
solid foo
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn missing_endloop_is_an_error() {
        // After 3 vertices we expect `endloop`; here we get `endfacet`
        // directly.
        let buf = b"\
solid foo
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn parser_continues_past_a_degenerate_facet() {
        // 3 facets in order: valid, degenerate (collinear), valid. The parser
        // must keep reading past the degenerate facet so an isolated bad
        // triangle in a multi-facet file does not fail the whole load.
        let buf = b"\
solid foo
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
  facet normal 0 0 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 2 0 0
    endloop
  endfacet
  facet normal 1 0 0
    outer loop
      vertex 0 0 0
      vertex 0 1 0
      vertex 0 0 1
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        let graph = read_stla(&mut reader).unwrap();
        let mesh = unwrap_single_mesh(&graph);

        // All 3 facets survive, with the normals they declare.
        assert_eq!(mesh.faces().len(), 3);
        assert_eq!(
            declared_normals(mesh),
            [[0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]
        );

        // The validator flags exactly the middle facet.
        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_face_areas()
            .expect_err("middle facet is degenerate");
        assert_eq!(
            errors,
            vec![sc_mesh_core::indexed_mesh::ZeroAreaFace {
                face_idx: 1_usize.into(),
            }]
        );
    }

    #[test]
    fn zero_normal_with_degenerate_triangle_is_preserved_for_validation() {
        // A collinear triangle that declares the zero normal: the parser keeps
        // the facet and `validate_face_areas` flags it later. Bailing here
        // would fail the whole file over a single bad triangle in an
        // otherwise-valid mesh.
        let buf = b"\
solid foo
  facet normal 0 0 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 2 0 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        let graph = read_stla(&mut reader).unwrap();
        let mesh = unwrap_single_mesh(&graph);

        assert_eq!(declared_normals(mesh), [[0.0, 0.0, 0.0]]);

        // The validator catches the degeneracy.
        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_face_areas()
            .expect_err("collinear triangle is degenerate");
        assert!(matches!(
            errors.as_slice(),
            [sc_mesh_core::indexed_mesh::ZeroAreaFace { .. }]
        ));
    }
}

#[cfg(test)]
mod binary_stl_malformed_tests {
    use super::*;

    fn make_binary_stl_with_count(count: u32, triangle_bytes: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(80 + 4 + triangle_bytes.len());
        buf.extend(std::iter::repeat_n(0u8, 80));
        buf.extend(&count.to_le_bytes());
        buf.extend(triangle_bytes);
        buf
    }

    #[test]
    fn truncated_before_header_is_an_error() {
        // Less than 80 bytes: the header read fails immediately.
        let buf = vec![0u8; 50];
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    #[test]
    fn truncated_before_triangle_count_is_an_error() {
        // 80 bytes of header but no triangle count.
        let buf = vec![0u8; 80];
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    #[test]
    fn declared_count_exceeds_actual_payload_is_an_error() {
        // The count field declares 3 triangles, but only 1 triangle record
        // follows, so the reader reports a truncated triangle record.
        let one_triangle = vec![0u8; 50];
        let buf = make_binary_stl_with_count(3, &one_triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    #[test]
    fn zero_count_yields_an_empty_mesh() {
        // Edge case but valid: a binary STL with no triangles. The reader must
        // succeed and produce a 1-leaf scene with an empty mesh rather than
        // fail on the missing payload.
        let buf = make_binary_stl_with_count(0, &[]);
        let mut reader = Cursor::new(buf);
        let (graph, _) = read_stlb(&mut reader).unwrap();
        let mesh = unwrap_single_mesh(&graph);
        assert!(mesh.faces().is_empty());
    }

    #[test]
    fn utf8_header_is_a_name() {
        // Non-ASCII text in the 80-byte header is a name like any other, as
        // long as it is valid UTF-8.
        let mut buf = vec![0u8; 80];
        buf[0..4].copy_from_slice(&[0xC3, 0xA9, 0xC3, 0xA9]); // "éé" in UTF-8
        buf.extend(&0u32.to_le_bytes());
        let mut reader = Cursor::new(buf);
        let (graph, warnings) = read_stlb(&mut reader).unwrap();
        assert!(warnings.is_empty());
        let mesh = unwrap_single_mesh(&graph);
        assert_eq!(
            mesh.meta.as_ref().and_then(|meta| meta.name.as_deref()),
            Some("éé"),
        );
    }

    #[test]
    fn invalid_utf8_header_loads_nameless() {
        // The header is a comment field; undecodable bytes must never fail the
        // load or become a name.
        let mut buf = vec![0u8; 80];
        buf[0..2].copy_from_slice(&[0xFF, 0xFE]);
        buf.extend(&0u32.to_le_bytes());
        let mut reader = Cursor::new(buf);
        let (graph, warnings) = read_stlb(&mut reader).unwrap();
        assert_eq!(warnings.len(), 1);
        let mesh = unwrap_single_mesh(&graph);
        assert!(
            mesh.meta
                .as_ref()
                .and_then(|meta| meta.name.as_deref())
                .is_none()
        );
    }

    #[test]
    fn parser_continues_past_a_degenerate_triangle() {
        // 3 faces in order: valid, degenerate (collinear), valid. The parser
        // must keep reading past the degenerate face so an isolated bad
        // triangle in a multi-face file does not fail the whole load.
        // Round-trip through `write_stlb` keeps the byte layout off the test
        // surface.
        use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
        use sc_mesh_formats::stl::write_stlb;

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([2.0, 0.0, 0.0]), // collinear with vertices 0 and 1
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![
                IndexedTriangle::from_raw_indices([0, 1, 2]),
                IndexedTriangle::from_raw_indices([0, 1, 3]),
                IndexedTriangle::from_raw_indices([0, 2, 4]),
            ],
        };
        let mut buf = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let (graph, _) = read_stlb(&mut reader).unwrap();
        let parsed = unwrap_single_mesh(&graph);

        // All 3 faces survive, with the normals that the writer gave them: the
        // zero vector for the degenerate face.
        assert_eq!(parsed.faces().len(), 3);
        assert_eq!(
            declared_normals(parsed),
            [[0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]
        );

        // The validator flags exactly the middle face, leaving the 2 valid
        // faces alone (the relative degeneracy threshold does not fire on
        // unit-edge triangles).
        let errors = parsed
            .validated_faces()
            .expect("face indices in range")
            .validate_face_areas()
            .expect_err("middle face is degenerate");
        assert_eq!(
            errors,
            vec![sc_mesh_core::indexed_mesh::ZeroAreaFace {
                face_idx: 1_usize.into(),
            }]
        );
    }

    #[test]
    fn zero_normal_with_degenerate_triangle_is_preserved_for_validation() {
        // Binary counterpart of the ASCII test: a triangle with 3 collinear
        // vertices, for which `write_stlb` writes the zero normal. The parser
        // keeps the facet so the file still loads, and `validate_face_areas`
        // flags the face later. Round-trip through `write_stlb` keeps the byte
        // layout off the test surface.
        use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
        use sc_mesh_formats::stl::write_stlb;

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([2.0, 0.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        };
        let mut buf = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let (graph, _) = read_stlb(&mut reader).unwrap();
        let parsed = unwrap_single_mesh(&graph);

        assert_eq!(declared_normals(parsed), [[0.0, 0.0, 0.0]]);

        // The validator catches the degeneracy.
        let errors = parsed
            .validated_faces()
            .expect("face indices in range")
            .validate_face_areas()
            .expect_err("collinear triangle is degenerate");
        assert!(matches!(
            errors.as_slice(),
            [sc_mesh_core::indexed_mesh::ZeroAreaFace { .. }]
        ));
    }

    /// Returns a well-formed 50-byte triangle record: a CCW triangle in the XY
    /// plane that declares a unit +Z normal. The non-finite-input tests change
    /// 1 f32 of it at a time.
    fn well_formed_triangle_bytes() -> Vec<u8> {
        let mut t = Vec::with_capacity(50);
        t.extend(&0.0_f32.to_le_bytes()); // normal.x
        t.extend(&0.0_f32.to_le_bytes()); // normal.y
        t.extend(&1.0_f32.to_le_bytes()); // normal.z
        t.extend(&0.0_f32.to_le_bytes()); // v0.x
        t.extend(&0.0_f32.to_le_bytes()); // v0.y
        t.extend(&0.0_f32.to_le_bytes()); // v0.z
        t.extend(&1.0_f32.to_le_bytes()); // v1.x
        t.extend(&0.0_f32.to_le_bytes()); // v1.y
        t.extend(&0.0_f32.to_le_bytes()); // v1.z
        t.extend(&0.0_f32.to_le_bytes()); // v2.x
        t.extend(&1.0_f32.to_le_bytes()); // v2.y
        t.extend(&0.0_f32.to_le_bytes()); // v2.z
        t.extend(&[0u8, 0u8]); // attribute byte count
        debug_assert_eq!(t.len(), 50);
        t
    }

    fn write_f32(triangle: &mut [u8], offset: usize, value: f32) {
        triangle[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Pins that NaN bit patterns in any f32 of a binary STL triangle record
    /// are rejected, as the finite-only check of the ASCII reader
    /// (`parse_finite_f32`) does. Each offset is a distinct field (normal.x to
    /// v2.z), so a failure names the field.
    #[test]
    fn nan_in_any_triangle_field_is_an_error() {
        for offset in [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44] {
            let mut triangle = well_formed_triangle_bytes();
            write_f32(&mut triangle, offset, f32::NAN);
            let buf = make_binary_stl_with_count(1, &triangle);
            let mut reader = Cursor::new(buf);
            assert!(
                read_stlb(&mut reader).is_err(),
                "binary STL with NaN at offset {offset} must be rejected",
            );
        }
    }

    /// Pins that positive and negative infinity in any f32 field of a binary
    /// STL triangle record are rejected.
    #[test]
    fn infinity_in_any_triangle_field_is_an_error() {
        for offset in [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44] {
            for infinity in [f32::INFINITY, f32::NEG_INFINITY] {
                let mut triangle = well_formed_triangle_bytes();
                write_f32(&mut triangle, offset, infinity);
                let buf = make_binary_stl_with_count(1, &triangle);
                let mut reader = Cursor::new(buf);
                assert!(
                    read_stlb(&mut reader).is_err(),
                    "binary STL with {infinity} at offset {offset} must be rejected",
                );
            }
        }
    }

    /// Pins that a triangle that declares the zero normal and has a non-finite
    /// vertex coordinate is rejected.
    #[test]
    fn zero_normal_with_nan_vertex_is_an_error() {
        let mut triangle = well_formed_triangle_bytes();
        write_f32(&mut triangle, 0, 0.0); // normal.x
        write_f32(&mut triangle, 4, 0.0); // normal.y
        write_f32(&mut triangle, 8, 0.0); // normal.z
        write_f32(&mut triangle, 16, f32::NAN); // v0.y
        let buf = make_binary_stl_with_count(1, &triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    /// Pins that a representative NaN bit-pattern that is *not* the canonical
    /// `f32::NAN` (different payload bits) is also rejected. Guards against a
    /// fix that compares the bit pattern against a single NaN encoding instead
    /// of using `is_finite`.
    #[test]
    fn non_canonical_nan_bit_pattern_in_normal_is_an_error() {
        // Quiet NaN with a non-zero payload, distinct from the bit pattern
        // produced by `f32::NAN`.
        let weird_nan = f32::from_bits(0x7FC0_1234);
        debug_assert!(weird_nan.is_nan());
        let mut triangle = well_formed_triangle_bytes();
        write_f32(&mut triangle, 0, weird_nan); // normal.x
        let buf = make_binary_stl_with_count(1, &triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    /// Pins that subnormal (denormalised) f32 values, which are finite per
    /// IEEE-754, are still accepted with the finite-only guard in place.
    #[test]
    fn subnormal_components_are_accepted() {
        let mut triangle = well_formed_triangle_bytes();
        // Subnormal: very small, but finite.
        write_f32(&mut triangle, 12, f32::from_bits(1)); // v0.x
        let buf = make_binary_stl_with_count(1, &triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_ok());
    }
}
