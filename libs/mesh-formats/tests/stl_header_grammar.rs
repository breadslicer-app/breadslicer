/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Integration tests for ASCII STL header acceptance through the
 * auto-detecting `read_stl` entry point, pinned to real vendored
 * files where they exist. The header grammar accepts `solid` as a
 * keyword (case-insensitive, optional leading spaces/tabs) followed
 * by an optional free-text name; the probe must route CRLF and
 * odd-name ASCII documents to the ASCII parser, while binary files
 * whose 80-byte header happens to start with `solid` keep routing to
 * the binary parser.
 */

use std::io::Cursor;

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Scene, Vertex};
use sc_mesh_formats::error::LoadStlError;
use sc_mesh_formats::stl::{read_stl, read_stla, write_stlb};

fn single_leaf_mesh(graph: &Scene) -> &IndexedMesh {
    assert_eq!(graph.reachable_leaf_count(), 1);
    let id = graph
        .first_leaf_id()
        .expect("reachable_leaf_count == 1 implies a leaf exists");
    graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
}

/// A one-facet, well-formed ASCII STL body under the given header
/// line. Only the header varies across these tests.
fn doc_with_header(header: &str) -> String {
    format!(
        "{header}\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\n\
         vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid\n"
    )
}

/// The exact byte transform a Windows text-mode write applies.
fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

fn read_stl_bytes(bytes: &[u8]) -> Result<Scene, LoadStlError> {
    read_stl(&mut Cursor::new(bytes))
}

fn read_stla_bytes(bytes: &[u8]) -> Result<Scene, LoadStlError> {
    read_stla(&mut Cursor::new(bytes))
}

fn loaded_name(scene: &Scene) -> Option<String> {
    single_leaf_mesh(scene)
        .meta
        .as_ref()
        .and_then(|meta| meta.name.clone())
}

#[cfg(test)]
mod stl_autodetect_header_tests {
    use super::*;

    #[test]
    fn crlf_ascii_document_loads() {
        let scene = read_stl_bytes(&crlf(&doc_with_header("solid cube"))).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn dotted_name_loads_with_the_name() {
        let scene = read_stl_bytes(doc_with_header("solid Part.001").as_bytes()).unwrap();
        assert_eq!(loaded_name(&scene).as_deref(), Some("Part.001"));
    }

    #[test]
    fn bare_solid_loads_nameless() {
        let scene = read_stl_bytes(doc_with_header("solid").as_bytes()).unwrap();
        assert_eq!(loaded_name(&scene), None);
    }

    #[test]
    fn blender_legacy_free_text_header_loads() {
        let scene =
            read_stl_bytes(doc_with_header("solid Exported from Blender-2.79 (sub 0)").as_bytes())
                .unwrap();
        assert_eq!(
            loaded_name(&scene).as_deref(),
            Some("Exported from Blender-2.79 (sub 0)"),
        );
    }

    #[test]
    fn leading_space_header_loads() {
        let scene = read_stl_bytes(doc_with_header(" solid part").as_bytes()).unwrap();
        assert_eq!(loaded_name(&scene).as_deref(), Some("part"));
    }

    #[test]
    fn double_space_header_loads_with_trimmed_name() {
        let scene = read_stl_bytes(doc_with_header("solid  part").as_bytes()).unwrap();
        assert_eq!(loaded_name(&scene).as_deref(), Some("part"));
    }

    #[test]
    fn unterminated_header_alone_fails_ascii_side() {
        let err = read_stl_bytes(b"solid foobar").unwrap_err();
        assert!(matches!(
            err,
            LoadStlError::AsciiEofWhileExpecting {
                what: "facet or endsolid",
            },
        ));
    }

    #[test]
    fn plain_lf_header_still_loads() {
        let scene = read_stl_bytes(doc_with_header("solid cube").as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    /// A binary STL whose 80-byte header begins with `solid ` (the
    /// rest is NUL padding) must keep routing to the binary parser.
    fn binary_with_solid_prefixed_header(faces: Vec<IndexedTriangle>) -> Vec<u8> {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        let mesh = IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some(String::from("solid part1")),
                ..Default::default()
            }),
            vertices,
            faces,
        };
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();
        assert_eq!(&out[..11], b"solid part1");
        out
    }

    #[test]
    fn binary_file_with_solid_prefixed_header_routes_to_binary() {
        let bytes = binary_with_solid_prefixed_header(vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )]);
        let scene = read_stl_bytes(&bytes).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn empty_binary_file_with_solid_prefixed_header_routes_to_binary() {
        let bytes = binary_with_solid_prefixed_header(vec![]);
        let scene = read_stl_bytes(&bytes).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 0);
    }
}

#[cfg(test)]
mod vendored_fixture_tests {
    use super::*;

    /// Loads one vendored fixture through both entry points and pins
    /// its facet count (counted once from the file at vendoring time).
    fn assert_fixture_loads(bytes: &[u8], faces: usize) {
        let via_auto = read_stl_bytes(bytes).unwrap();
        assert_eq!(single_leaf_mesh(&via_auto).faces().len(), faces);
        let via_ascii = read_stla_bytes(bytes).unwrap();
        assert_eq!(single_leaf_mesh(&via_ascii).faces().len(), faces);
    }

    #[test]
    fn flat_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/flat.stl"),
            3,
        );
    }

    #[test]
    fn cube_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/cube.stl"),
            12,
        );
    }

    #[test]
    fn block100_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/block100.stl"),
            12,
        );
    }

    #[test]
    fn humanoid_free_text_name_loads() {
        let bytes = include_bytes!("fixtures/official/burkardt-stla/humanoid.stl");
        assert_fixture_loads(bytes, 96);
        let scene = read_stl_bytes(bytes).unwrap();
        assert_eq!(
            loaded_name(&scene).as_deref(),
            Some("MYSOLID created by IVCON, original data in file.obj"),
        );
    }

    #[test]
    fn sphere_trailing_space_header_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/sphere.stl"),
            228,
        );
    }

    #[test]
    fn bottle_leading_space_header_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/bottle.stl"),
            1240,
        );
    }

    #[test]
    fn teapot_leading_space_header_loads() {
        assert_fixture_loads(
            include_bytes!("fixtures/official/burkardt-stla/teapot.stl"),
            2016,
        );
    }

    #[test]
    fn flat_with_crlf_line_endings_loads() {
        let text =
            String::from_utf8(include_bytes!("fixtures/official/burkardt-stla/flat.stl").to_vec())
                .expect("vendored ASCII file");
        let scene = read_stl_bytes(&crlf(&text)).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 3);
    }
}
