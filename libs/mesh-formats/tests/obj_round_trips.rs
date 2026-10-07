/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use float_cmp::approx_eq;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_mesh_formats::messages::LoadObjWarning;
use sc_mesh_formats::obj::{read_obj, write_obj};
use sc_scene::{NodeId, NodeKind, Scene};
use sc_scene_fixtures::meshes_as_roots;

fn make_triangle_mesh(name: Option<&str>) -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([0.0, 0.0, 1.0]),
    ];
    let normal =
        compute_normal_from_triangle_vertices(&[vertices[0], vertices[1], vertices[2]]).unwrap();
    IndexedMesh::new(
        name.map(|n| MeshMetadata {
            name: Some(n.to_string()),
            ..Default::default()
        }),
        vertices,
        vec![IndexedTriangle::from_raw_indices(normal, [0, 1, 2])],
    )
}

fn round_trip(graph: &Scene) -> (Scene, Vec<LoadObjWarning>) {
    let mut buf: Vec<u8> = Vec::new();
    write_obj(graph, &mut buf).expect("write_obj failed");

    let mut reader = std::io::Cursor::new(buf);
    read_obj(&mut reader).expect("read_obj failed")
}

fn unwrap_single_mesh(graph: &Scene) -> &IndexedMesh {
    assert_eq!(graph.reachable_leaf_count(), 1);
    let id = graph
        .first_leaf_id()
        .expect("reachable_leaf_count == 1 implies a leaf exists");
    graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
}

fn collect_leaf_ids(graph: &Scene) -> Vec<NodeId> {
    let mut ids = Vec::new();
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => ids.push(id),
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for c in children.iter().rev() {
                    stack.push(*c);
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    ids
}

fn assert_mesh_vertices_eq(a: &IndexedMesh, b: &IndexedMesh) {
    assert_eq!(a.vertices().len(), b.vertices().len());
    for (i, (va, vb)) in a.vertices().iter().zip(b.vertices().iter()).enumerate() {
        for c in 0..3 {
            assert!(
                approx_eq!(f32, va[c], vb[c], ulps = 2),
                "vertex {i} coord {c}: {} != {}",
                va[c],
                vb[c],
            );
        }
    }
}

fn assert_mesh_faces_eq(a: &IndexedMesh, b: &IndexedMesh) {
    assert_eq!(a.faces().len(), b.faces().len());
    for (i, (fa, fb)) in a.faces().iter().zip(b.faces().iter()).enumerate() {
        assert_eq!(fa.vertices, fb.vertices, "face {i} vertex indices differ");
        for c in 0..3 {
            assert!(
                approx_eq!(f32, fa.normal[c], fb.normal[c], ulps = 4),
                "face {i} normal coord {c}: {} != {}",
                fa.normal[c],
                fb.normal[c],
            );
        }
    }
}

#[test]
fn single_unnamed_mesh_round_trip() {
    let mesh = make_triangle_mesh(None);
    let graph = meshes_as_roots(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn single_named_mesh_round_trip() {
    let mesh = make_triangle_mesh(Some("TestObj"));
    let graph = meshes_as_roots(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_eq!(
        recovered_mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref()),
        Some("TestObj"),
    );
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn two_objects_round_trip() {
    let mesh_a = make_triangle_mesh(Some("A"));
    let mesh_b = IndexedMesh::new(
        Some(MeshMetadata {
            name: Some("B".to_string()),
            ..Default::default()
        }),
        vec![
            Vertex::new([2.0, 0.0, 0.0]),
            Vertex::new([0.0, 2.0, 0.0]),
            Vertex::new([0.0, 0.0, 2.0]),
        ],
        vec![IndexedTriangle::from_raw_indices(
            compute_normal_from_triangle_vertices(&[
                Vertex::new([2.0, 0.0, 0.0]),
                Vertex::new([0.0, 2.0, 0.0]),
                Vertex::new([0.0, 0.0, 2.0]),
            ])
            .unwrap(),
            [0, 1, 2],
        )],
    );
    let graph = meshes_as_roots(vec![mesh_a.clone(), mesh_b.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let leaf_ids = collect_leaf_ids(&recovered);
    assert_eq!(leaf_ids.len(), 2);

    for (original, leaf_id) in [&mesh_a, &mesh_b].iter().zip(leaf_ids) {
        let recovered_mesh = recovered.mesh_at_leaf(leaf_id).expect("leaf has a mesh");
        assert_mesh_vertices_eq(original, recovered_mesh);
        assert_mesh_faces_eq(original, recovered_mesh);
    }
}

#[test]
fn empty_scene_round_trip() {
    let graph = Scene::new();
    let (recovered, warnings) = round_trip(&graph);

    assert_eq!(recovered.reachable_leaf_count(), 0);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, LoadObjWarning::NoVertices))
    );
}

#[test]
fn newline_name_round_trips_folded() {
    let mesh = make_triangle_mesh(Some("a\nb"));
    let graph = meshes_as_roots(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_eq!(
        recovered_mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref()),
        Some("a b"),
    );
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn directive_injection_name_round_trips_with_identical_geometry() {
    // A newline inside a name must not become document structure. Unfolded,
    // this name adds a `v 9 9 9` line, and its extra vertex shifts every face
    // index that the writer computed: the file loads back without an error,
    // with the wrong triangle.
    let mesh = make_triangle_mesh(Some("Injected\nv 9 9 9"));
    let graph = meshes_as_roots(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_eq!(
        recovered_mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref()),
        Some("Injected v 9 9 9"),
    );
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn han_name_round_trips_transliterated() {
    let mesh = make_triangle_mesh(Some("深圳"));
    let graph = meshes_as_roots(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&graph);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_eq!(
        recovered_mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref()),
        Some("ShenZhen"),
    );
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}
