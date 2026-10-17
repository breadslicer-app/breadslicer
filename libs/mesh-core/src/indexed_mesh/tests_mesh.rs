/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Vertex};

#[test]
fn new_keeps_its_parts() {
    let meta = MeshMetadata {
        name: Some("part".to_owned()),
        ..MeshMetadata::default()
    };
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
    ];
    let faces = vec![IndexedTriangle::from_raw_indices([0, 1, 2])];

    let mesh = IndexedMesh::new(Some(meta.clone()), vertices.clone(), faces.clone());

    assert_eq!(mesh.meta(), &Some(meta));
    assert_eq!(mesh.vertices(), vertices.as_slice());
    assert_eq!(mesh.faces(), faces.as_slice());
}

#[test]
fn local_aabb_of_a_mesh_without_vertices_is_none() {
    assert!(
        IndexedMesh::new(None, vec![], vec![])
            .local_aabb()
            .is_none()
    );
}

#[test]
fn local_aabb_spans_the_extremes_of_every_axis() {
    // The 1st vertex is at neither extreme on any axis.
    let mesh = IndexedMesh::new(
        None,
        vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([-1.0, 2.0, -3.0]),
            Vertex::new([4.0, -5.0, 6.0]),
        ],
        vec![],
    );

    let aabb = mesh.local_aabb().unwrap();

    assert_eq!(aabb.min(), Vertex::new([-1.0, -5.0, -3.0]));
    assert_eq!(aabb.max(), Vertex::new([4.0, 2.0, 6.0]));
}
