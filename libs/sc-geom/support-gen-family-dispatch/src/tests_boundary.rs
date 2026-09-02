/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_mesh_core::{IndexedMesh, Vector3D};

use crate::boundary::BoundaryShape;

fn unit_box() -> BoundaryShape {
    BoundaryShape::BuildVolumeBox {
        min: Vector3D::new([0.0, 0.0, 0.0]),
        max: Vector3D::new([10.0, 10.0, 10.0]),
    }
}

#[test]
fn ray_straight_down_exits_at_the_floor() {
    let hit = unit_box()
        .cast_ray(
            Vector3D::new([5.0, 5.0, 8.0]),
            Vector3D::new([0.0, 0.0, -1.0]),
        )
        .expect("ray must hit the box");
    assert!((hit.point.coords()[2] - 0.0).abs() < 1e-9);
    assert!((hit.distance - 8.0).abs() < 1e-9);
}

#[test]
fn ray_along_an_axis_exits_at_the_correct_face() {
    let hit = unit_box()
        .cast_ray(
            Vector3D::new([2.0, 2.0, 2.0]),
            Vector3D::new([1.0, 0.0, 0.0]),
        )
        .expect("ray must hit the box");
    assert!((hit.point.coords()[0] - 10.0).abs() < 1e-9);
    assert!((hit.distance - 8.0).abs() < 1e-9);
}

#[test]
fn ray_from_outside_moving_away_never_hits() {
    // Origin is outside the box on Z, and the ray moves further away.
    assert!(
        unit_box()
            .cast_ray(
                Vector3D::new([5.0, 5.0, 20.0]),
                Vector3D::new([0.0, 0.0, 1.0])
            )
            .is_none()
    );
}

#[test]
fn mesh_boundary_does_not_support_ray_queries_yet() {
    let mesh = IndexedMesh::new(None, Vec::new(), Vec::new());
    let boundary = BoundaryShape::Mesh(mesh);
    assert!(
        boundary
            .cast_ray(
                Vector3D::new([0.0, 0.0, 0.0]),
                Vector3D::new([0.0, 0.0, -1.0])
            )
            .is_none()
    );
}
