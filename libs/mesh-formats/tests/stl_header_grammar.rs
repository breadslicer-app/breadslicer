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

/// Reads through the auto-detecting entry, keeping only the scene;
/// tests that care about warnings go through `load_mesh_from_reader`
/// in `stl_binary_header_tests` instead.
fn read_stl_bytes(bytes: &[u8]) -> Result<Scene, LoadStlError> {
    read_stl(&mut Cursor::new(bytes)).map(|(scene, _)| scene)
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
#[cfg(test)]
mod stl_binary_header_tests {
    use super::*;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::messages::{LoadMeshWarning, LoadStlWarning};
    use sc_mesh_formats::{LoadedMesh, load_mesh_from_reader};

    /// One 50-byte binary record: +Z normal, unit XY triangle, zero
    /// attribute bytes.
    fn triangle_record() -> Vec<u8> {
        let mut record = Vec::with_capacity(50);
        for value in [
            0.0_f32, 0.0, 1.0, // normal
            0.0, 0.0, 0.0, // v0
            1.0, 0.0, 0.0, // v1
            0.0, 1.0, 0.0, // v2
        ] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        record.extend_from_slice(&0_u16.to_le_bytes());
        record
    }

    /// A binary document with the given header bytes (NUL-padded to
    /// 80) declaring and carrying 1 valid triangle record. Only the
    /// header varies across these tests.
    fn binary_doc_with_header(header: &[u8]) -> Vec<u8> {
        assert!(header.len() <= 80);
        let mut doc = vec![0_u8; 80];
        doc[..header.len()].copy_from_slice(header);
        doc.extend_from_slice(&1_u32.to_le_bytes());
        doc.extend_from_slice(&triangle_record());
        doc
    }

    fn load(kind: MeshFileType, bytes: &[u8]) -> LoadedMesh {
        load_mesh_from_reader(kind, &mut Cursor::new(bytes)).expect("well-formed geometry loads")
    }

    fn header_not_text_warnings(loaded: &LoadedMesh) -> usize {
        loaded
            .warnings
            .iter()
            .filter(|w| matches!(w, LoadMeshWarning::Stl(LoadStlWarning::HeaderNotText)))
            .count()
    }

    /// The Materialise Magics colour convention places `COLOR=` plus
    /// 4 raw RGBA bytes in the header; 0xFF can never appear in valid
    /// UTF-8, so the header is not text. The geometry must load, with
    /// no name and exactly one header warning.
    #[test]
    fn magics_color_header_loads_geometry() {
        let mut header = b"COLOR=".to_vec();
        header.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]);
        for kind in [MeshFileType::Stl, MeshFileType::StlBin] {
            let loaded = load(kind, &binary_doc_with_header(&header));
            assert_eq!(single_leaf_mesh(&loaded.graph).faces().len(), 1);
            assert_eq!(loaded_name(&loaded.graph), None);
            assert_eq!(header_not_text_warnings(&loaded), 1);
        }
    }

    /// Windows-codepage text (0xFC is u-umlaut in Windows-1252) is
    /// not valid UTF-8 and is not transcoded: nameless load with a
    /// header warning.
    #[test]
    fn codepage_header_loads_nameless() {
        let loaded = load(MeshFileType::StlBin, &binary_doc_with_header(b"K\xFCche"));
        assert_eq!(single_leaf_mesh(&loaded.graph).faces().len(), 1);
        assert_eq!(loaded_name(&loaded.graph), None);
        assert_eq!(header_not_text_warnings(&loaded), 1);
    }

    /// ASCII control bytes decode as UTF-8 but are not printable
    /// text; they must not become a mesh name.
    #[test]
    fn control_junk_header_loads_nameless() {
        let junk: Vec<u8> = [0x07_u8, 0x01].iter().copied().cycle().take(20).collect();
        let loaded = load(MeshFileType::StlBin, &binary_doc_with_header(&junk));
        assert_eq!(single_leaf_mesh(&loaded.graph).faces().len(), 1);
        assert_eq!(loaded_name(&loaded.graph), None);
        assert_eq!(header_not_text_warnings(&loaded), 1);
    }

    /// Valid UTF-8 headers are text: the name is taken verbatim,
    /// non-ASCII included, with no warning.
    #[test]
    fn utf8_header_yields_unicode_name() {
        let loaded = load(
            MeshFileType::StlBin,
            &binary_doc_with_header("Küche".as_bytes()),
        );
        assert_eq!(loaded_name(&loaded.graph).as_deref(), Some("Küche"));
        assert!(loaded.warnings.is_empty());
    }

    /// The 80-byte header needs no NUL terminator: a name filling the
    /// full width is taken whole.
    #[test]
    fn full_width_ascii_name_no_nul() {
        let loaded = load(MeshFileType::StlBin, &binary_doc_with_header(&[b'a'; 80]));
        assert_eq!(loaded_name(&loaded.graph), Some("a".repeat(80)));
        assert!(loaded.warnings.is_empty());
    }

    /// The common all-zero header is plain namelessness, not a
    /// warning condition.
    #[test]
    fn zero_header_stays_silent() {
        let loaded = load(MeshFileType::StlBin, &binary_doc_with_header(&[]));
        assert_eq!(loaded_name(&loaded.graph), None);
        assert!(loaded.warnings.is_empty());
    }

    /// Whitespace padding before the NULs trims to no name; benign,
    /// so no warning either.
    #[test]
    fn whitespace_header_stays_silent() {
        let loaded = load(MeshFileType::StlBin, &binary_doc_with_header(b"    "));
        assert_eq!(loaded_name(&loaded.graph), None);
        assert!(loaded.warnings.is_empty());
    }
}
