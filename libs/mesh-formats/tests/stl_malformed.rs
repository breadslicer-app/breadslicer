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
        // No leading "solid " — the regex rejects this on the very
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
        // `facet NOTNORMAL 0 0 0` — the second token has to be the
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
        let scene = read_stla(&mut reader).unwrap();
        // The recomputed normal for a CCW triangle in the XY plane is
        // (0, 0, 1).
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        let n = mesh.faces[0].normal;
        assert!((n[0]).abs() < 1e-6);
        assert!((n[1]).abs() < 1e-6);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_normal_with_degenerate_triangle_is_an_error() {
        // Same shape as the previous test but with a collinear triangle:
        // the recomputation returns an error, which the reader propagates.
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
        assert!(read_stla(&mut reader).is_err());
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
        // Less than 80 bytes — the header read fails immediately.
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
        let scene = read_stlb(&mut reader).unwrap();
        assert_eq!(scene.objects.len(), 1);
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        assert!(mesh.faces.is_empty());
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
        let scene = read_stlb(&mut reader).unwrap();
        let mesh = match &scene.objects[0].content {
            sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
            _ => panic!("expected an inline mesh"),
        };
        let n = mesh.faces[0].normal;
        assert!((n[0]).abs() < 1e-6);
        assert!((n[1]).abs() < 1e-6);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }
}
