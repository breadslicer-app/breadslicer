/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use float_cmp::approx_eq;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_mesh_formats::obj::{read_obj, write_obj};

fn make_triangle_mesh(name: Option<&str>) -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([0.0, 0.0, 1.0]),
    ];
    let normal =
        compute_normal_from_triangle_vertices(&[vertices[0], vertices[1], vertices[2]]).unwrap();
    IndexedMesh {
        meta: name.map(|n| MeshMetadata {
            name: Some(n.to_string()),
            ..Default::default()
        }),
        vertices,
        faces: vec![IndexedTriangle {
            normal,
            vertices: [0, 1, 2],
        }],
    }
}

fn make_scene(objects: Vec<IndexedMesh>) -> Scene {
    Scene {
        meta: None,
        resources: vec![],
        objects: objects
            .into_iter()
            .map(|mesh| SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(mesh),
            })
            .collect(),
    }
}

fn round_trip(scene: &Scene) -> (Scene, Vec<LoadMeshWarning>) {
    let mut buf: Vec<u8> = Vec::new();
    write_obj(scene, &mut buf).expect("write_obj failed");

    let mut reader = std::io::Cursor::new(buf);
    let mut warnings = vec![];
    let recovered = read_obj(&mut reader, &mut warnings).expect("read_obj failed");
    (recovered, warnings)
}

fn unwrap_single_mesh(scene: &Scene) -> &IndexedMesh {
    assert_eq!(scene.objects.len(), 1);
    match &scene.objects[0].content {
        SceneObjectContent::Mesh(m) => m,
        _ => panic!("Expected inline mesh"),
    }
}

fn assert_mesh_vertices_eq(a: &IndexedMesh, b: &IndexedMesh) {
    assert_eq!(a.vertices.len(), b.vertices.len());
    for (i, (va, vb)) in a.vertices.iter().zip(b.vertices.iter()).enumerate() {
        for c in 0..3 {
            assert!(
                approx_eq!(f32, va.0[c], vb.0[c], ulps = 2),
                "vertex {i} coord {c}: {} != {}",
                va.0[c],
                vb.0[c],
            );
        }
    }
}

fn assert_mesh_faces_eq(a: &IndexedMesh, b: &IndexedMesh) {
    assert_eq!(a.faces.len(), b.faces.len());
    for (i, (fa, fb)) in a.faces.iter().zip(b.faces.iter()).enumerate() {
        assert_eq!(fa.vertices, fb.vertices, "face {i} vertex indices differ");
        for c in 0..3 {
            assert!(
                approx_eq!(f32, fa.normal.0[c], fb.normal.0[c], ulps = 4),
                "face {i} normal coord {c}: {} != {}",
                fa.normal.0[c],
                fb.normal.0[c],
            );
        }
    }
}

#[test]
fn single_unnamed_mesh_round_trip() {
    let mesh = make_triangle_mesh(None);
    let scene = make_scene(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&scene);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn single_named_mesh_round_trip() {
    let mesh = make_triangle_mesh(Some("TestObj"));
    let scene = make_scene(vec![mesh.clone()]);
    let (recovered, warnings) = round_trip(&scene);

    assert!(warnings.is_empty());
    let recovered_mesh = unwrap_single_mesh(&recovered);
    assert_eq!(
        recovered_mesh.meta.as_ref().and_then(|m| m.name.as_deref()),
        Some("TestObj"),
    );
    assert_mesh_vertices_eq(&mesh, recovered_mesh);
    assert_mesh_faces_eq(&mesh, recovered_mesh);
}

#[test]
fn two_objects_round_trip() {
    let mesh_a = make_triangle_mesh(Some("A"));
    let mesh_b = IndexedMesh {
        meta: Some(MeshMetadata {
            name: Some("B".to_string()),
            ..Default::default()
        }),
        vertices: vec![
            Vertex::new([2.0, 0.0, 0.0]),
            Vertex::new([0.0, 2.0, 0.0]),
            Vertex::new([0.0, 0.0, 2.0]),
        ],
        faces: vec![IndexedTriangle {
            normal: compute_normal_from_triangle_vertices(&[
                Vertex::new([2.0, 0.0, 0.0]),
                Vertex::new([0.0, 2.0, 0.0]),
                Vertex::new([0.0, 0.0, 2.0]),
            ])
            .unwrap(),
            vertices: [0, 1, 2],
        }],
    };
    let scene = make_scene(vec![mesh_a.clone(), mesh_b.clone()]);
    let (recovered, warnings) = round_trip(&scene);

    assert!(warnings.is_empty());
    assert_eq!(recovered.objects.len(), 2);

    for (original, obj) in [&mesh_a, &mesh_b].iter().zip(recovered.objects.iter()) {
        let recovered_mesh = match &obj.content {
            SceneObjectContent::Mesh(m) => m,
            _ => panic!("Expected inline mesh"),
        };
        assert_mesh_vertices_eq(original, recovered_mesh);
        assert_mesh_faces_eq(original, recovered_mesh);
    }
}

#[test]
fn empty_scene_round_trip() {
    let scene = Scene::default();
    let (recovered, warnings) = round_trip(&scene);

    assert!(recovered.objects.is_empty());
    assert!(warnings.iter().any(|w| matches!(
        w,
        LoadMeshWarning::Obj(sc_mesh_formats::messages::LoadObjWarning::NoVertices)
    )));
}
