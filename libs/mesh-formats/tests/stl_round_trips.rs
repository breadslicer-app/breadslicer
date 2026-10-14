/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_mesh_fixtures::with_declared_facet_normals;
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};
use sc_scene::Scene;

/// Returns the mesh of the only leaf of `graph`: STL readers wrap the on-disk
/// mesh in a single-leaf graph (the `From<IndexedMesh>` impl of `Scene`), and
/// round-trip tests compare the recovered `IndexedMesh` to the input directly.
fn single_leaf_mesh(graph: &Scene) -> &IndexedMesh {
    assert_eq!(graph.reachable_leaf_count(), 1);
    let id = graph
        .first_leaf_id()
        .expect("reachable_leaf_count == 1 implies a leaf exists");
    graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
}

fn get_mesh_without_meta() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 100.0, 100.0]),
            Vertex::new([0.0, 100.0, 0.0]),
            Vertex::new([0.0, 0.0, 100.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    }
}

fn get_unnamed_mesh() -> IndexedMesh {
    let mesh = IndexedMesh {
        meta: Some(MeshMetadata::default()),
        vertices: vec![
            Vertex::new([0.0, 100.0, 100.0]),
            Vertex::new([0.0, 100.0, 0.0]),
            Vertex::new([0.0, 0.0, 100.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    with_declared_facet_normals(mesh, vec![[0xbf80_0000, 0, 0]])
}

/// Returns the mesh that a write of [`get_mesh_without_meta`] reads back as: no
/// name, and the normal the writers compute for its face declared.
fn read_back_mesh_without_meta() -> IndexedMesh {
    with_declared_facet_normals(
        get_mesh_without_meta(),
        vec![[0xbf80_0000, 0x8000_0000, 0x8000_0000]],
    )
}

fn get_named_mesh() -> IndexedMesh {
    get_mesh_named("my_mesh")
}

fn get_mesh_named(name: &str) -> IndexedMesh {
    let mesh = IndexedMesh {
        meta: Some(MeshMetadata {
            name: Some(String::from(name)),
            ..Default::default()
        }),
        vertices: vec![
            Vertex::new([0.0, 100.0, 100.0]),
            Vertex::new([0.0, 100.0, 0.0]),
            Vertex::new([0.0, 0.0, 100.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
    };
    with_declared_facet_normals(mesh, vec![[0xbf80_0000, 0, 0]])
}

/// Writes the mesh as ASCII STL and reads it back, returning the recovered mesh
/// name.
fn stla_round_tripped_name(mesh: &IndexedMesh) -> Option<String> {
    let mut out: Vec<u8> = vec![];
    write_stla(mesh, &mut out).expect("write_stla succeeds");
    let mut reader = std::io::Cursor::new(out);
    let graph = read_stla(&mut reader).expect("written document parses back");
    single_leaf_mesh(&graph)
        .meta
        .as_ref()
        .and_then(|meta| meta.name.clone())
}

#[cfg(test)]
mod stla_tests {
    use super::*;

    #[test]
    fn test_mesh_to_stla_to_mesh_no_meta() {
        let mesh_without_meta = get_mesh_without_meta();
        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_without_meta, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let recovered_graph = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &read_back_mesh_without_meta());
    }

    #[test]
    fn test_mesh_to_stla_to_mesh_unnamed() {
        let mesh_unnamed = get_unnamed_mesh();

        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_unnamed, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let recovered_graph = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stla_to_mesh_named() {
        let mesh_named = get_named_mesh();

        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_named, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let recovered_graph = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &mesh_named);
    }

    #[test]
    fn dotted_name_round_trips() {
        let name = stla_round_tripped_name(&get_mesh_named("Part.001"));
        assert_eq!(name.as_deref(), Some("Part.001"));
    }

    #[test]
    fn multi_word_name_round_trips() {
        let name = stla_round_tripped_name(&get_mesh_named("Part A"));
        assert_eq!(name.as_deref(), Some("Part A"));
    }

    #[test]
    fn newline_in_name_is_normalized_on_write_and_round_trips() {
        // The writer collapses whitespace runs (line breaks included) into
        // single spaces, so the document stays well-formed and the recovered
        // name is the normalized form.
        let name = stla_round_tripped_name(&get_mesh_named("a\nb"));
        assert_eq!(name.as_deref(), Some("a b"));
    }

    #[test]
    fn unicode_name_round_trips_transliterated() {
        // The writer emits ASCII-only names; the umlaut folds to its base
        // letter. The reader itself accepts non-ASCII names (ingestion stays
        // tolerant), so this pins the writer alone.
        let name = stla_round_tripped_name(&get_mesh_named("Küche"));
        assert_eq!(name.as_deref(), Some("Kuche"));
    }
}

#[cfg(test)]
mod stlb_tests {
    use super::*;

    #[test]
    fn test_mesh_to_stlb_to_mesh_no_meta() {
        let mesh_without_meta = get_mesh_without_meta();
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_without_meta, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let (recovered_graph, _) = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &read_back_mesh_without_meta());
    }

    #[test]
    fn test_mesh_to_stlb_to_mesh_unnamed() {
        let mesh_unnamed = get_unnamed_mesh();

        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_unnamed, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let (recovered_graph, _) = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stlb_to_mesh_named() {
        let mesh_named = get_named_mesh();

        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_named, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            panic!("Error: {:?}", recovered_mesh.err().unwrap());
        }
        let (recovered_graph, _) = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &mesh_named);
    }
}
#[cfg(test)]
mod stlb_header_roundtrip_tests {
    use super::*;

    use sc_mesh_formats::filetypes::MeshFileType;
    use sc_mesh_formats::{LoadedMesh, load_mesh_from_reader};

    fn stlb_write_then_load(mesh: &IndexedMesh) -> LoadedMesh {
        let mut out: Vec<u8> = vec![];
        write_stlb(mesh, &mut out).expect("write_stlb succeeds");
        load_mesh_from_reader(MeshFileType::StlBin, &mut std::io::Cursor::new(out))
            .expect("written document loads back")
    }

    fn loaded_name(loaded: &LoadedMesh) -> Option<String> {
        single_leaf_mesh(&loaded.graph)
            .meta
            .as_ref()
            .and_then(|meta| meta.name.clone())
    }

    #[test]
    fn unicode_name_round_trips_transliterated_through_stlb() {
        // The writer emits ASCII-only names; the umlaut folds to its base
        // letter.
        let loaded = stlb_write_then_load(&get_mesh_named("Küche"));
        assert_eq!(loaded_name(&loaded).as_deref(), Some("Kuche"));
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn overlong_name_folds_to_exactly_80_bytes_and_survives() {
        // 79 ASCII bytes plus a 2-byte char: folded, the accented char becomes
        // its base letter and the name fits the 80-byte header exactly. A raw
        // byte cut would split the char in 2 and give a non-text header, which
        // loads as a nameless mesh with a warning.
        let loaded = stlb_write_then_load(&get_mesh_named(&format!("{}é", "a".repeat(79))));
        assert_eq!(single_leaf_mesh(&loaded.graph).faces().len(), 1);
        assert_eq!(
            loaded_name(&loaded).as_deref(),
            Some(format!("{}e", "a".repeat(79)).as_str()),
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn long_ascii_name_truncates_silently_to_the_80_byte_header() {
        // The 80-byte cap is the format's; the cut is deliberate and carries no
        // warning (there is no write-warning channel).
        let loaded = stlb_write_then_load(&get_mesh_named(&"a".repeat(100)));
        assert_eq!(
            loaded_name(&loaded).as_deref(),
            Some("a".repeat(80).as_str())
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn tab_name_round_trips_collapsed() {
        // Unfolded, the tab is a control character in the header, which loads
        // as a nameless mesh with the header warning; folded, it collapses to a
        // space, as in the ASCII STL writer.
        let loaded = stlb_write_then_load(&get_mesh_named("a\tb"));
        assert_eq!(loaded_name(&loaded).as_deref(), Some("a b"));
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn nul_name_round_trips_with_the_nul_removed() {
        // Unfolded, the NUL acts as the header terminator and cuts the name to
        // "a" without a warning.
        let loaded = stlb_write_then_load(&get_mesh_named("a\0b"));
        assert_eq!(loaded_name(&loaded).as_deref(), Some("ab"));
        assert!(loaded.warnings.is_empty());
    }
}
