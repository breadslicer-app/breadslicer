/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! write_stlb -> read_stlb round-trip coverage: shared fixtures plus a
//! generated torus, pinning that the slice-based reader reproduces the written
//! geometry bit for bit.

use std::io::Cursor;

use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{IndexedMesh, MeshMetadata, Triangle, Vertex, indexed_mesh_from_soup};
use sc_mesh_fixtures::{
    cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh, with_declared_facet_normals,
};

use crate::stl::{read_stlb, write_stlb};

/// Round-trips `mesh` through write_stlb and the public read_stlb entry,
/// returning the single leaf mesh of the resulting scene.
fn round_trip(mesh: &IndexedMesh) -> IndexedMesh {
    let mut buf: Vec<u8> = Vec::new();
    write_stlb(mesh, &mut buf).expect("write_stlb succeeds");
    let mut cursor = Cursor::new(buf);
    let (scene, warnings) = read_stlb(&mut cursor).expect("read_stlb succeeds");
    assert!(warnings.is_empty());
    let leaf = scene.first_leaf_id().expect("one leaf");
    scene
        .mesh_at_leaf(leaf)
        .expect("leaf resolves to a mesh")
        .clone()
}

/// Returns, for each face, the coordinate bit patterns of its 3 vertices,
/// independent of vertex-list order.
fn resolved_faces(mesh: &IndexedMesh) -> Vec<[[u32; 3]; 3]> {
    mesh.faces()
        .iter()
        .map(|face| {
            face.vertices
                .map(|idx| mesh.vertices()[usize::from(idx)].coords().map(f32::to_bits))
        })
        .collect()
}

/// Asserts that `mesh` round-trips: the STL soup format keeps per-face
/// geometry, so a round-trip must preserve every resolved face bit for bit and,
/// for fixtures without duplicate vertices, dedup back to the same vertex
/// count. A second round-trip must then be an exact fixed point.
fn assert_fixture_round_trips(mesh: IndexedMesh) {
    let first = round_trip(&mesh);
    assert_eq!(resolved_faces(&first), resolved_faces(&mesh));
    assert_eq!(first.vertices().len(), mesh.vertices().len());

    let second = round_trip(&first);
    assert_eq!(second, first);
}

#[test]
fn unit_cube_round_trips() {
    assert_fixture_round_trips(unit_cube_mesh());
}

#[test]
fn z_tunnel_prism_round_trips() {
    assert_fixture_round_trips(cuboid_with_z_tunnel());
}

#[test]
fn y_tunnel_prism_round_trips() {
    assert_fixture_round_trips(cuboid_with_y_tunnel());
}

/// Returns a generated torus with its normals declared: n x m quads, vertices
/// shared across rings and segments. Built through `indexed_mesh_from_soup`, so
/// its vertex order is first-appearance order and the round-trip must reproduce
/// the full `IndexedMesh` exactly, metadata included.
fn torus_mesh(n: usize, m: usize) -> IndexedMesh {
    let (major, minor) = (10.0_f32, 3.0_f32);
    let point = |i: usize, j: usize| -> Vertex {
        let u = (i % n) as f32 / n as f32 * std::f32::consts::TAU;
        let v = (j % m) as f32 / m as f32 * std::f32::consts::TAU;
        Vertex::new([
            (major + minor * v.cos()) * u.cos(),
            (major + minor * v.cos()) * u.sin(),
            minor * v.sin(),
        ])
    };
    let mut soup: Vec<Triangle> = Vec::with_capacity(2 * n * m);
    let mut normals: Vec<[u32; 3]> = Vec::with_capacity(2 * n * m);
    for i in 0..n {
        for j in 0..m {
            let quads = [
                [point(i, j), point(i + 1, j), point(i + 1, j + 1)],
                [point(i, j), point(i + 1, j + 1), point(i, j + 1)],
            ];
            for vertices in quads {
                let normal = compute_normal_from_triangle_vertices(&vertices)
                    .expect("torus faces are not degenerate");
                normals.push(normal.coords().map(f32::to_bits));
                soup.push(Triangle::new(vertices));
            }
        }
    }
    let meta = MeshMetadata {
        name: Some("torus".to_string()),
        ..Default::default()
    };
    with_declared_facet_normals(
        indexed_mesh_from_soup(Some(meta), soup, u32::MAX as usize).expect("torus builds"),
        normals,
    )
}

#[test]
fn generated_torus_round_trips_exactly() {
    // 768 faces: exercises the sequential record-parse path.
    let mesh = torus_mesh(24, 16);
    assert_eq!(round_trip(&mesh), mesh);
}

#[test]
fn large_generated_torus_round_trips_exactly() {
    // 8640 faces: exercises the parallel record-parse path, whose output must
    // stay positionally identical to the sequential one.
    const { assert!(2 * 72 * 60 > crate::stl::stlb::PARALLEL_PARSE_MIN_RECORDS) };
    let mesh = torus_mesh(72, 60);
    assert_eq!(round_trip(&mesh), mesh);
}
