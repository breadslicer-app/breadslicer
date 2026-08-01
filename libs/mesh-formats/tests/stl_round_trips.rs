/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Scene, Vertex};
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};

/// STL readers wrap the on-disk mesh in a single-leaf graph
/// (`Scene::from<IndexedMesh>`). Pull that mesh back out by
/// reading the first reachable leaf; round-trip tests then compare
/// the recovered `IndexedMesh` to the input directly.
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
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([-1.0, 0.0, 0.0]),
            [0, 1, 2],
        )],
    }
}

fn get_unnamed_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: Some(MeshMetadata::default()),
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

fn get_named_mesh() -> IndexedMesh {
    get_mesh_named("my_mesh")
}

fn get_mesh_named(name: &str) -> IndexedMesh {
    IndexedMesh {
        meta: Some(MeshMetadata {
            name: Some(String::from(name)),
            ..Default::default()
        }),
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

/// Writes the mesh as ASCII STL and reads it back, returning the
/// recovered mesh name.
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

        // Even if there is no metadata loaded, we still create the Metadata object.
        let mesh_unnamed = get_unnamed_mesh();
        assert_eq!(recovered_mesh, &mesh_unnamed);
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
        // The writer collapses whitespace runs (line breaks included)
        // into single spaces, so the document stays well-formed and
        // the recovered name is the normalized form.
        let name = stla_round_tripped_name(&get_mesh_named("a\nb"));
        assert_eq!(name.as_deref(), Some("a b"));
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
        let recovered_graph = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        // Even if there is no metadata loaded, we still create the Metadata object.
        let mesh_unnamed = get_unnamed_mesh();
        assert_eq!(recovered_mesh, &mesh_unnamed);
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
        let recovered_graph = recovered_mesh.unwrap();
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
        let recovered_graph = recovered_mesh.unwrap();
        let recovered_mesh = single_leaf_mesh(&recovered_graph);

        assert_eq!(recovered_mesh, &mesh_named);
    }
}
