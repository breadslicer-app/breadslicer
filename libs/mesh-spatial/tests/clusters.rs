/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Build correctness for [`TriangleClusters`].

use sc_mesh_core::{IndexedTriangle, Normal, Vertex};
use sc_mesh_spatial::TriangleClusters;

fn vertex(x: f32, y: f32, z: f32) -> Vertex {
    Vertex::new([x, y, z])
}

fn tri(a: u32, b: u32, c: u32) -> IndexedTriangle {
    IndexedTriangle {
        normal: Normal::new([0.0, 0.0, 1.0]),
        vertices: [a, b, c],
    }
}

#[test]
fn build_returns_none_for_empty_faces() {
    let vertices = vec![vertex(0.0, 0.0, 0.0)];
    let faces: Vec<IndexedTriangle> = vec![];
    assert!(TriangleClusters::build(&vertices, &faces).is_none());
}

#[test]
fn build_returns_none_for_out_of_range_face_index() {
    let vertices = vec![vertex(0.0, 0.0, 0.0)];
    let faces = vec![tri(0, 1, 2)];
    assert!(TriangleClusters::build(&vertices, &faces).is_none());
}

#[test]
fn single_face_yields_single_cluster_with_face_aabb() {
    let vertices = vec![
        vertex(-1.0, -2.0, 0.0),
        vertex(3.0, -2.0, 0.0),
        vertex(-1.0, 5.0, 7.0),
    ];
    let faces = vec![tri(0, 1, 2)];
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters.face_count(), 1);
    let a = clusters.aabb(0);
    assert_eq!(a.min().coords(), [-1.0, -2.0, 0.0]);
    assert_eq!(a.max().coords(), [3.0, 5.0, 7.0]);
}

#[test]
fn cluster_count_is_ceil_face_count_over_cluster_size() {
    let vertices = vec![vertex(0.0, 0.0, 0.0)];
    let faces = vec![tri(0, 0, 0); (TriangleClusters::CLUSTER_SIZE as usize) + 1];
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    assert_eq!(clusters.len(), 2);
    assert_eq!(clusters.face_count(), faces.len() as u32);
}

#[test]
fn iter_emits_half_open_face_ranges_in_cluster_order() {
    let vertices = vec![vertex(0.0, 0.0, 0.0)];
    let face_count = (TriangleClusters::CLUSTER_SIZE as usize) + 5;
    let faces = vec![tri(0, 0, 0); face_count];
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    let collected: Vec<_> = clusters.iter().map(|(r, _)| r).collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0], 0..TriangleClusters::CLUSTER_SIZE);
    assert_eq!(
        collected[1],
        TriangleClusters::CLUSTER_SIZE..(face_count as u32)
    );
}

#[test]
fn cluster_aabb_only_covers_faces_in_that_cluster() {
    // Two clusters: first cluster faces all sit at z = 0, second
    // cluster faces all sit at z = 100. The cluster AABBs must
    // separate cleanly along z.
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let cs = TriangleClusters::CLUSTER_SIZE as usize;
    for _ in 0..cs {
        let base = vertices.len() as u32;
        vertices.push(vertex(0.0, 0.0, 0.0));
        vertices.push(vertex(1.0, 0.0, 0.0));
        vertices.push(vertex(0.0, 1.0, 0.0));
        faces.push(tri(base, base + 1, base + 2));
    }
    for _ in 0..3 {
        let base = vertices.len() as u32;
        vertices.push(vertex(0.0, 0.0, 100.0));
        vertices.push(vertex(1.0, 0.0, 100.0));
        vertices.push(vertex(0.0, 1.0, 100.0));
        faces.push(tri(base, base + 1, base + 2));
    }
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    assert_eq!(clusters.len(), 2);
    assert_eq!(clusters.aabb(0).max().coords()[2], 0.0);
    assert_eq!(clusters.aabb(1).min().coords()[2], 100.0);
}
