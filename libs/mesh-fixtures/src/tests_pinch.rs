/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Self-tests for the deliberately vertex-pinched fixtures
//! ([`crate::bowtie_tetrahedra`] and
//! [`crate::cuboid_with_pinched_z_tunnel`]).
//!
//! These pin the combinatorial contract the fixtures advertise:
//! edge-manifold and consistently wound (the validator mints its
//! witness), closed (no boundary edges), the expected vertex, face,
//! edge and Euler counts, the pinch vertex's incident-face count,
//! and the edge-connected component structure that tells the 2
//! fixtures apart (2 lobes vs 1 strangled loop). Vertex-fan
//! structure itself is asserted in `sc-mesh-core`'s topology tests,
//! not here.

use std::collections::HashMap;

use sc_mesh_core::IndexedMesh;

use crate::{bowtie_tetrahedra, cuboid_with_pinched_z_tunnel};

/// Count distinct undirected edges across all faces.
fn undirected_edge_count(mesh: &IndexedMesh) -> usize {
    let mut edges = std::collections::HashSet::new();
    for face in &mesh.faces {
        let [a, b, c] = face.vertices;
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (u, v) = (usize::from(u), usize::from(v));
            edges.insert(if u <= v { (u, v) } else { (v, u) });
        }
    }
    edges.len()
}

/// Number of faces incident to the given vertex index.
fn incident_face_count(mesh: &IndexedMesh, vertex: u32) -> usize {
    mesh.faces
        .iter()
        .filter(|face| {
            face.vertices
                .iter()
                .any(|&v| usize::from(v) == vertex as usize)
        })
        .count()
}

/// Number of edge-connected face components: faces are joined when
/// they share an undirected edge.
fn edge_connected_component_count(mesh: &IndexedMesh) -> usize {
    let mut edge_to_faces: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (idx, face) in mesh.faces.iter().enumerate() {
        let [a, b, c] = face.vertices;
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (u, v) = (usize::from(u), usize::from(v));
            let key = if u <= v { (u, v) } else { (v, u) };
            edge_to_faces.entry(key).or_default().push(idx);
        }
    }

    let mut parent: Vec<usize> = (0..mesh.faces.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cursor = x;
        while parent[cursor] != root {
            let next = parent[cursor];
            parent[cursor] = root;
            cursor = next;
        }
        root
    }
    for faces in edge_to_faces.values() {
        for pair in faces.windows(2) {
            let (a, b) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            parent[a] = b;
        }
    }
    (0..mesh.faces.len())
        .map(|idx| find(&mut parent, idx))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

#[test]
fn bowtie_has_expected_counts_and_euler_characteristic() {
    let mesh = bowtie_tetrahedra();
    assert_eq!(mesh.vertices.len(), 7);
    assert_eq!(mesh.faces.len(), 8);
    let edges = undirected_edge_count(&mesh);
    assert_eq!(edges, 12);
    // Two spheres sharing a point: chi = 2 + 2 - 1.
    assert_eq!(mesh.vertices.len() + mesh.faces.len() - edges, 3);
}

#[test]
fn bowtie_mints_the_edge_level_manifold_witness() {
    let mesh = bowtie_tetrahedra();
    let faces = mesh
        .validated_faces()
        .expect("bowtie faces must be structurally valid");
    faces
        .validated_edge_manifold()
        .expect("bowtie must pass edge-level manifold validation");
}

#[test]
fn bowtie_pinch_vertex_joins_two_edge_connected_lobes() {
    let mesh = bowtie_tetrahedra();
    assert_eq!(incident_face_count(&mesh, 3), 6);
    assert_eq!(edge_connected_component_count(&mesh), 2);
    // Every face belongs to exactly one lobe's vertex set; the lobes
    // overlap only in the pinch vertex.
    let lower: std::collections::HashSet<usize> = [0, 1, 2, 3].into();
    let upper: std::collections::HashSet<usize> = [3, 4, 5, 6].into();
    for face in &mesh.faces {
        let used: std::collections::HashSet<usize> =
            face.vertices.iter().map(|&v| usize::from(v)).collect();
        assert!(
            used.is_subset(&lower) || used.is_subset(&upper),
            "face {used:?} straddles the lobes"
        );
    }
}

#[test]
fn pinched_tunnel_has_expected_counts_and_euler_characteristic() {
    let mesh = cuboid_with_pinched_z_tunnel();
    assert_eq!(mesh.vertices.len(), 17);
    assert_eq!(mesh.faces.len(), 32);
    let edges = undirected_edge_count(&mesh);
    assert_eq!(edges, 48);
    // Pinched torus: chi = 1.
    assert_eq!(mesh.vertices.len() + mesh.faces.len() - edges, 1);
}

#[test]
fn pinched_tunnel_mints_the_edge_level_manifold_witness() {
    let mesh = cuboid_with_pinched_z_tunnel();
    let faces = mesh
        .validated_faces()
        .expect("pinched tunnel faces must be structurally valid");
    faces
        .validated_edge_manifold()
        .expect("pinched tunnel must pass edge-level manifold validation");
}

#[test]
fn pinched_tunnel_is_one_strangled_component() {
    let mesh = cuboid_with_pinched_z_tunnel();
    assert_eq!(incident_face_count(&mesh, 16), 8);
    // Unlike the bowtie, the pinch strangles a single edge-connected
    // surface: no decomposition into separate bodies exists.
    assert_eq!(edge_connected_component_count(&mesh), 1);
}
