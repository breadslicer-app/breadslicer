/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::geometry::compute_normal_from_triangle_vertices;
use crate::indexed_mesh::IndexedMesh;
use crate::primitives::{IndexedTriangle, Vertex};

/// Build an axis-aligned box from its 12 outward-wound triangles.
fn box_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
    let corners = [
        [min[0], min[1], min[2]],
        [max[0], min[1], min[2]],
        [max[0], max[1], min[2]],
        [min[0], max[1], min[2]],
        [min[0], min[1], max[2]],
        [max[0], min[1], max[2]],
        [max[0], max[1], max[2]],
        [min[0], max[1], max[2]],
    ];
    let vertices: Vec<Vertex> = corners.into_iter().map(Vertex::new).collect();
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ]
    .into_iter()
    .map(|indices: [u32; 3]| {
        let corner = |i: u32| vertices[i as usize];
        let normal = compute_normal_from_triangle_vertices(&[
            corner(indices[0]),
            corner(indices[1]),
            corner(indices[2]),
        ])
        .expect("a box face is never degenerate");
        IndexedTriangle::from_raw_indices(normal, indices)
    })
    .collect();

    IndexedMesh::new(None, vertices, faces)
}

/// Reverse every face's winding, leaving the geometry alone.
fn inverted(mesh: &IndexedMesh) -> IndexedMesh {
    let faces = mesh
        .faces
        .iter()
        .map(|face| {
            let [a, b, c] = face.vertices;
            IndexedTriangle {
                normal: face.normal,
                vertices: [c, b, a],
            }
        })
        .collect();
    IndexedMesh::new(None, mesh.vertices.clone(), faces)
}

fn volume_of(mesh: &IndexedMesh) -> f64 {
    mesh.validated_faces()
        .expect("the fixture's indices are in range")
        .signed_volume_mm3()
}

#[test]
fn a_unit_cube_encloses_one_cubic_millimetre() {
    assert_eq!(volume_of(&box_mesh([0.0; 3], [1.0; 3])), 1.0);
}

#[test]
fn a_box_encloses_the_product_of_its_extents() {
    assert_eq!(volume_of(&box_mesh([0.0; 3], [2.0, 3.0, 5.0])), 30.0);
}

#[test]
fn inward_winding_reports_the_negative_of_the_same_volume() {
    let mesh = box_mesh([0.0; 3], [2.0, 3.0, 5.0]);
    assert_eq!(volume_of(&inverted(&mesh)), -volume_of(&mesh));
}

#[test]
fn translating_a_box_leaves_its_volume_alone() {
    // Each term of the sum grows with distance from the origin while
    // the total must not, which is why the sum is taken in f64.
    let here = volume_of(&box_mesh([0.0; 3], [1.0; 3]));
    let far = volume_of(&box_mesh([1000.0; 3], [1001.0; 3]));
    assert!(
        (far - here).abs() < 1e-6,
        "at the origin {here}, a metre away {far}",
    );
}

#[test]
fn an_empty_mesh_encloses_nothing() {
    assert_eq!(volume_of(&IndexedMesh::new(None, vec![], vec![])), 0.0);
}

#[test]
fn a_tetrahedron_encloses_a_sixth_of_its_bounding_corner() {
    // Corner tetrahedron on the axes: volume is (1/6)*a*b*c.
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([3.0, 0.0, 0.0]),
        Vertex::new([0.0, 4.0, 0.0]),
        Vertex::new([0.0, 0.0, 5.0]),
    ];
    let faces = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]]
        .into_iter()
        .map(|indices: [u32; 3]| {
            let normal = compute_normal_from_triangle_vertices(&[
                vertices[indices[0] as usize],
                vertices[indices[1] as usize],
                vertices[indices[2] as usize],
            ])
            .expect("the fixture's faces are never degenerate");
            IndexedTriangle::from_raw_indices(normal, indices)
        })
        .collect();

    let mesh = IndexedMesh::new(None, vertices, faces);
    assert!((volume_of(&mesh) - 10.0).abs() < 1e-9);
}
