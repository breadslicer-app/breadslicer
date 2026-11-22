/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Build correctness for [`TriangleClusters`].

use sc_mesh_core::{IndexedTriangle, Vertex};
use sc_mesh_spatial::TriangleClusters;

fn vertex(x: f32, y: f32, z: f32) -> Vertex {
    Vertex::new([x, y, z])
}

fn tri(a: u32, b: u32, c: u32) -> IndexedTriangle {
    IndexedTriangle::from_raw_indices([a, b, c])
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
fn every_face_is_in_one_cluster_and_every_cluster_but_the_last_is_full() {
    let (vertices, faces) = wavy_sheet(37);
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    let size = TriangleClusters::CLUSTER_SIZE as usize;
    let sizes: Vec<usize> = clusters.iter().map(|(f, _)| f.len()).collect();
    assert_eq!(sizes.len(), faces.len().div_ceil(size));
    assert!(sizes[..sizes.len() - 1].iter().all(|&n| n == size));
    let mut every: Vec<u32> = clusters.iter().flat_map(|(f, _)| f.to_vec()).collect();
    every.sort_unstable();
    assert_eq!(every, (0..faces.len() as u32).collect::<Vec<_>>());
}

/// Faces that come scattered through the file still make tight clusters:
/// each cluster's box holds its faces' area and little more.
#[test]
fn scattered_faces_make_tight_clusters() {
    let (vertices, mut faces) = wavy_sheet(80);
    // A fixed shuffle, as a file written out of order has.
    let n = faces.len();
    for i in 0..n {
        let j = (i * 7919 + 13) % n;
        faces.swap(i, j);
    }
    let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
    let widest = clusters
        .iter()
        .map(|(_, b)| {
            let (lo, hi) = (b.min().coords(), b.max().coords());
            (hi[0] - lo[0]).max(hi[1] - lo[1])
        })
        .fold(0.0f32, f32::max);
    // 64 triangles of a 1 mm grid: a box of some 8 mm at the most.
    assert!(
        widest <= 12.0,
        "widest cluster {widest} mm on an 80 mm sheet"
    );
}

#[test]
fn cluster_aabb_only_covers_faces_in_that_cluster() {
    // 2 clusters: the faces of the first cluster all sit at z = 0, the faces of
    // the second at z = 100. The cluster AABBs must separate cleanly along z.
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

/// A grid of small triangles over a wavy sheet, enough for a deep tree.
fn wavy_sheet(n: u32) -> (Vec<Vertex>, Vec<IndexedTriangle>) {
    let mut vertices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32, j as f32);
            vertices.push(vertex(x, y, (x * 0.3).sin() * 4.0 + (y * 0.2).cos() * 3.0));
        }
    }
    let at = |i: u32, j: u32| j * (n + 1) + i;
    let mut faces = Vec::new();
    for j in 0..n {
        for i in 0..n {
            faces.push(tri(at(i, j), at(i + 1, j), at(i + 1, j + 1)));
            faces.push(tri(at(i, j), at(i + 1, j + 1), at(i, j + 1)));
        }
    }
    (vertices, faces)
}

/// The tree walk reaches every cluster a query reaches, as testing each
/// box in turn does, and the nearest it finds is the same.
#[test]
fn the_tree_walk_finds_what_testing_every_box_finds() {
    let (vertices, faces) = wavy_sheet(120);
    let clusters = TriangleClusters::build(&vertices, &faces).expect("clusters");
    // A query: boxes that reach the slab x in [lo, hi], how far each
    // starts above z = -10; a cluster's "hit" is how far above its box
    // starts, plus a little per first face so they differ.
    for (lo, hi) in [(10.0, 12.0), (0.0, 120.0), (55.5, 56.0), (200.0, 300.0)] {
        let enter = |b: &sc_mesh_core::Aabb| {
            let (min, max) = (b.min().coords(), b.max().coords());
            (max[0] >= lo && min[0] <= hi).then_some(min[2] + 10.0)
        };
        let hit_of = |cluster: &[u32]| {
            let (_, b) = clusters
                .iter()
                .find(|(f, _)| f[0] == cluster[0])
                .expect("a cluster");
            b.min().coords()[2] + 10.0 + cluster[0] as f32 * 1e-6
        };
        let mut every: Vec<u32> = clusters
            .iter()
            .filter(|(_, b)| enter(b).is_some())
            .map(|(f, _)| f[0])
            .collect();
        let mut walked = Vec::new();
        clusters.walk_nearest(enter, |cluster, _| {
            walked.push(cluster[0]);
            None
        });
        every.sort_unstable();
        walked.sort_unstable();
        assert_eq!(walked, every, "every cluster reached, for [{lo}, {hi}]");

        let nearest = clusters
            .iter()
            .filter(|(_, b)| enter(b).is_some())
            .map(|(f, _)| hit_of(f))
            .fold(f32::INFINITY, f32::min);
        let mut best = f32::INFINITY;
        clusters.walk_nearest(enter, |cluster, _| {
            let hit = hit_of(cluster);
            best = best.min(hit);
            Some(hit)
        });
        assert_eq!(best, nearest, "the nearest, for [{lo}, {hi}]");
    }
}
