/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

use crate::dispatch::{normalize, suggest_support_regions};

fn single_triangle_mesh(normal: [f32; 3]) -> IndexedMesh {
    IndexedMesh::new(
        Some(MeshMetadata::default()),
        vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        vec![IndexedTriangle::from_raw_indices(
            Normal::new(normal),
            [0, 1, 2],
        )],
    )
}

#[test]
fn normalize_scales_to_unit_length() {
    let n = normalize(sc_mesh_core::Vector3D::new([3.0, 4.0, 0.0])).unwrap();
    assert!((n.norm() - 1.0).abs() < 1e-9);
}

#[test]
fn normalize_rejects_the_zero_vector() {
    assert!(normalize(sc_mesh_core::Vector3D::new([0.0, 0.0, 0.0])).is_none());
}

#[test]
fn a_downward_facing_triangle_scores_high_under_downward_gravity() {
    let mesh = single_triangle_mesh([0.0, 0.0, -1.0]);
    let gravity = sc_mesh_core::Vector3D::new([0.0, 0.0, -1.0]);
    let scores = suggest_support_regions(&mesh, gravity);
    assert_eq!(scores.len(), 3);
    for score in scores {
        assert!(score > 0.9, "expected a high score, got {score}");
    }
}

#[test]
fn an_upward_facing_triangle_scores_zero_under_downward_gravity() {
    let mesh = single_triangle_mesh([0.0, 0.0, 1.0]);
    let gravity = sc_mesh_core::Vector3D::new([0.0, 0.0, -1.0]);
    let scores = suggest_support_regions(&mesh, gravity);
    for score in scores {
        assert_eq!(score, 0.0);
    }
}

#[test]
fn an_empty_mesh_yields_an_empty_score_vector() {
    let mesh = IndexedMesh::new(None, Vec::new(), Vec::new());
    let gravity = sc_mesh_core::Vector3D::new([0.0, 0.0, -1.0]);
    assert!(suggest_support_regions(&mesh, gravity).is_empty());
}
