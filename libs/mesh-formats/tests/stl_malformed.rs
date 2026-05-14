/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Malformed-input integration tests for the STL readers. Pins the
 * error branches of `stla.rs` and `stlb.rs` against fixture buffers
 * so future refactors can't silently swap a hard error for a default
 * value (or vice-versa).
 *
 * Companion to `stl_round_trips.rs`, which covers the happy paths.
 */

use std::io::Cursor;

use sc_mesh_formats::stl::{read_stla, read_stlb};

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
        // No leading "solid ": the regex rejects this on the very
        // first line.
        let buf = b"facet normal 0 0 0\nendsolid\n";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn truncated_after_solid_is_an_error() {
        // Header is fine, but the file ends before any facet appears
        // and without an `endsolid` either.
        let buf = b"solid foo\n";
        let mut reader = Cursor::new(buf.to_vec());
        assert!(read_stla(&mut reader).is_err());
    }

    #[test]
    fn invalid_facet_header_is_an_error() {
        // `facet NOTNORMAL 0 0 0`: the second token has to be the
        // literal `normal`.
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
        // "NaN" parses as a float but is rejected by the finite-only
        // guard inside `tokens_to_f32`.
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
        // After `facet normal …` the next line must be `outer loop`.
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
        // Only two coordinates supplied for the first vertex.
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
        // The facet starts but the file ends before all three vertices
        // and the closing `endloop` / `endfacet` are read.
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
        // After three vertices we expect `endloop`; here we get
        // `endfacet` directly.
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
    fn zero_normal_is_recomputed_from_vertices() {
        // The reader interprets a zero-magnitude normal as a hint to
        // recompute the normal from the vertices. This exercises the
        // `compute_normal_from_triangle_vertices` fallback path inside
        // `next_face`.
        let buf = b"\
solid foo
  facet normal 0 0 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid foo
";
        let mut reader = Cursor::new(buf.to_vec());
        let scene = read_stla(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        // The recomputed normal for a CCW triangle in the XY plane is
        // (0, 0, 1).
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        let n = mesh.faces()[0].normal;
        assert!((n[0]).abs() < 1e-6);
        assert!((n[1]).abs() < 1e-6);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parser_continues_past_a_degenerate_facet() {
        // Three facets in order: valid, degenerate (collinear), valid.
        // The parser must keep reading past the degenerate facet so an
        // isolated bad triangle in a multi-facet file doesn't fail the
        // whole load.
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
        let scene = read_stla(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };

        // All three facets survive; only the middle one carries a zero
        // normal (the recompute couldn't produce a direction).
        assert_eq!(mesh.faces().len(), 3);
        assert_eq!(mesh.faces()[0].normal.coords(), [0.0_f32, 0.0_f32, 1.0_f32]);
        assert_eq!(mesh.faces()[1].normal.coords(), [0.0_f32, 0.0_f32, 0.0_f32]);
        assert_eq!(mesh.faces()[2].normal.coords(), [1.0_f32, 0.0_f32, 0.0_f32]);

        // The validator flags exactly the middle facet.
        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("middle facet is degenerate");
        assert_eq!(
            errors,
            vec![
                sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle {
                    face_idx: 1_usize.into(),
                }
            ]
        );
    }

    #[test]
    fn zero_normal_with_degenerate_triangle_is_preserved_for_validation() {
        // Same shape as the previous test but with a collinear triangle.
        // The recomputation can't produce a meaningful direction; the
        // parser leaves the zero normal in place and `validate_normal_vectors`
        // flags the face later. Bailing here would fail the whole file
        // over a single bad triangle in an otherwise-valid mesh.
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
        let scene = read_stla(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };

        // The zero normal is preserved verbatim.
        let n = mesh.faces()[0].normal;
        assert_eq!(n.coords(), [0.0_f32, 0.0_f32, 0.0_f32]);

        // The validator catches the degeneracy.
        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("collinear triangle is degenerate");
        assert!(matches!(
            errors.as_slice(),
            [sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle { .. }]
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
        // Header + count says 3 triangles, but only one triangle's
        // worth of bytes follows. The iterator hits UnexpectedEof on
        // the second triangle.
        let one_triangle = vec![0u8; 50];
        let buf = make_binary_stl_with_count(3, &one_triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    #[test]
    fn zero_count_yields_an_empty_mesh() {
        // Edge case but valid: a binary STL with no triangles. The
        // reader must succeed and produce an empty scene rather than
        // erroring on the missing payload.
        let buf = make_binary_stl_with_count(0, &[]);
        let mut reader = Cursor::new(buf);
        let scene = read_stlb(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        assert_eq!(scene.objects.len(), 1);
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        assert!(mesh.faces().is_empty());
    }

    #[test]
    fn non_ascii_header_is_an_error() {
        // Place a few non-ASCII bytes inside the 80-byte header. The
        // metadata extractor errors out before reading any triangles.
        let mut buf = vec![0u8; 80];
        buf[0..4].copy_from_slice(&[0xC3, 0xA9, 0xC3, 0xA9]); // "éé" in UTF-8
        buf.extend(&0u32.to_le_bytes());
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    #[test]
    fn zero_normal_is_recomputed_from_vertices() {
        // Same shape as the ASCII case: a triangle with a zero normal
        // and three non-collinear vertices. The reader recomputes the
        // normal as (0, 0, 1).
        let mut triangle = Vec::with_capacity(50);
        triangle.extend(&0.0_f32.to_le_bytes()); // normal.x
        triangle.extend(&0.0_f32.to_le_bytes()); // normal.y
        triangle.extend(&0.0_f32.to_le_bytes()); // normal.z
        triangle.extend(&0.0_f32.to_le_bytes()); // v0.x
        triangle.extend(&0.0_f32.to_le_bytes()); // v0.y
        triangle.extend(&0.0_f32.to_le_bytes()); // v0.z
        triangle.extend(&1.0_f32.to_le_bytes()); // v1.x
        triangle.extend(&0.0_f32.to_le_bytes()); // v1.y
        triangle.extend(&0.0_f32.to_le_bytes()); // v1.z
        triangle.extend(&0.0_f32.to_le_bytes()); // v2.x
        triangle.extend(&1.0_f32.to_le_bytes()); // v2.y
        triangle.extend(&0.0_f32.to_le_bytes()); // v2.z
        triangle.extend(&[0u8, 0u8]); // attribute byte count

        let buf = make_binary_stl_with_count(1, &triangle);
        let mut reader = Cursor::new(buf);
        let scene = read_stlb(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        let n = mesh.faces()[0].normal;
        assert!((n[0]).abs() < 1e-6);
        assert!((n[1]).abs() < 1e-6);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parser_continues_past_a_degenerate_triangle() {
        // Three faces in order: valid, degenerate (collinear), valid.
        // The parser must keep reading past the degenerate face so an
        // isolated bad triangle in a multi-face file doesn't fail the
        // whole load. Round-trip through `write_stlb` keeps the byte
        // layout off the test surface.
        use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};
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
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]),
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 0.0]), [0, 1, 3]),
                IndexedTriangle::from_raw_indices(Normal::new([1.0, 0.0, 0.0]), [0, 2, 4]),
            ],
        };
        let mut buf = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let scene = read_stlb(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        let parsed = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };

        // All three faces survive; only the middle one carries a zero
        // normal (the recompute couldn't produce a direction). The two
        // valid faces keep their on-disk normals verbatim.
        assert_eq!(parsed.faces().len(), 3);
        assert_eq!(
            parsed.faces()[0].normal.coords(),
            [0.0_f32, 0.0_f32, 1.0_f32]
        );
        assert_eq!(
            parsed.faces()[1].normal.coords(),
            [0.0_f32, 0.0_f32, 0.0_f32]
        );
        assert_eq!(
            parsed.faces()[2].normal.coords(),
            [1.0_f32, 0.0_f32, 0.0_f32]
        );

        // The validator flags exactly the middle face, leaving the two
        // valid faces alone (Part 1's relative threshold doesn't fire on
        // unit-edge triangles).
        let errors = parsed
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("middle face is degenerate");
        assert_eq!(
            errors,
            vec![
                sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle {
                    face_idx: 1_usize.into(),
                }
            ]
        );
    }

    #[test]
    fn zero_normal_with_degenerate_triangle_is_preserved_for_validation() {
        // Binary counterpart of the ASCII test: a triangle with the
        // zero-normal sentinel and three collinear vertices. The
        // recomputation can't produce a meaningful direction; the parser
        // keeps the zero normal so the file still loads, and
        // `validate_normal_vectors` flags the face later. Round-trip
        // through `write_stlb` keeps the byte layout off the test surface.
        use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};
        use sc_mesh_formats::stl::write_stlb;

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([2.0, 0.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        };
        let mut buf = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let scene = read_stlb(&mut reader)
            .unwrap()
            .try_flatten()
            .expect("STL reader output flattens");
        let parsed = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };

        // The zero normal is preserved verbatim.
        let n = parsed.faces()[0].normal;
        assert_eq!(n.coords(), [0.0_f32, 0.0_f32, 0.0_f32]);

        // The validator catches the degeneracy.
        let errors = parsed
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("collinear triangle is degenerate");
        assert!(matches!(
            errors.as_slice(),
            [sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle { .. }]
        ));
    }

    /// Helpers for the non-finite-input regression tests.
    fn well_formed_triangle_bytes() -> Vec<u8> {
        // CCW triangle in the XY plane with a unit +Z normal: the
        // happy-path input the next few tests perturb, one f32 at a
        // time.
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

    /// MF-C-1 regression: NaN bit-patterns in any f32 of a binary STL
    /// triangle record must be rejected, mirroring the ASCII reader's
    /// `tokens_to_f32` finite-only guard. Each offset corresponds to a
    /// distinct field (normal.x/y/z, v0.x/y/z, v1.x/y/z, v2.x/y/z) so
    /// that any future regression is localised to a specific field.
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

    /// MF-C-1 regression: positive and negative infinity in any f32
    /// field of a binary STL triangle record must be rejected.
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

    /// MF-C-1 regression: a triangle whose stored normal is all-zero
    /// (the "recompute me" sentinel) but whose vertex coordinates are
    /// non-finite must still be rejected. Before the fix, the
    /// `normal == Normal::new([0,0,0])` shortcut would short-circuit
    /// the comparison while the NaN/Inf vertices propagated through
    /// `compute_normal_from_triangle_vertices` and into the mesh.
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

    /// MF-C-1 regression: a representative NaN bit-pattern that is
    /// *not* the canonical `f32::NAN` (different payload bits) must
    /// also be rejected. Guards against a fix that compares the bit
    /// pattern against a single NaN encoding instead of using
    /// `is_finite`.
    #[test]
    fn non_canonical_nan_bit_pattern_in_normal_is_an_error() {
        // Quiet NaN with a non-zero payload, distinct from the bit
        // pattern produced by `f32::NAN`.
        let weird_nan = f32::from_bits(0x7FC0_1234);
        debug_assert!(weird_nan.is_nan());
        let mut triangle = well_formed_triangle_bytes();
        write_f32(&mut triangle, 0, weird_nan); // normal.x
        let buf = make_binary_stl_with_count(1, &triangle);
        let mut reader = Cursor::new(buf);
        assert!(read_stlb(&mut reader).is_err());
    }

    /// MF-C-1 regression: subnormal (denormalised) f32 values are
    /// finite per IEEE-754 and must continue to be accepted, even
    /// after the finite-only guard is in place.
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
