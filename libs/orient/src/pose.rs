/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! From an "up" direction to a transform.

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::vec::{V3, basis, cross, dot, mul, unit};

/// The rotation (row-major) taking the unit vector `from` to the unit vector
/// `to` along the shortest arc, so whatever does not have to turn does not:
/// the model's front keeps facing where it faced. Opposite vectors turn half
/// a turn about an axis across `to`.
pub fn rotation_between(from: [f64; 3], to: [f64; 3]) -> [[f64; 3]; 3] {
    let (a, b) = (
        unit(from).unwrap_or([0.0, 0.0, 1.0]),
        unit(to).unwrap_or([0.0, 0.0, 1.0]),
    );
    let c = dot(a, b);
    if c < -1.0 + 1e-12 {
        let (axis, _) = basis(b);
        return axis_angle(axis, std::f64::consts::PI);
    }
    let k = cross(a, b); // axis × sin
    let f = 1.0 / (1.0 + c);
    [
        [
            c + k[0] * k[0] * f,
            k[0] * k[1] * f - k[2],
            k[0] * k[2] * f + k[1],
        ],
        [
            k[1] * k[0] * f + k[2],
            c + k[1] * k[1] * f,
            k[1] * k[2] * f - k[0],
        ],
        [
            k[2] * k[0] * f - k[1],
            k[2] * k[1] * f + k[0],
            c + k[2] * k[2] * f,
        ],
    ]
}

/// The rotation taking `up` to +Z along the shortest arc.
pub fn rotation_to_up(up: [f64; 3]) -> [[f64; 3]; 3] {
    rotation_between(up, [0.0, 0.0, 1.0])
}

/// The rotation (row-major) by `angle` radians about the unit `axis`.
pub(crate) fn axis_angle(axis: V3, angle: f64) -> [[f64; 3]; 3] {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

pub(crate) fn matmul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// The box of the model's vertices at `world`.
fn world_bounds(mesh: &IndexedMesh, world: &AffineTransform3D) -> ([f64; 3], [f64; 3]) {
    let (l, o) = (world.linear(), world.translation());
    mesh.vertices().iter().fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(mut lo, mut hi), v| {
            let p = mul(l, v.coords().map(f64::from));
            for k in 0..3 {
                lo[k] = lo[k].min(p[k] + o[k]);
                hi[k] = hi[k].max(p[k] + o[k]);
            }
            (lo, hi)
        },
    )
}

/// How far inside the plate's edges a shifted model stays, mm: the viewport
/// checks the envelope in f32, so a model shifted exactly to an edge could
/// read as crossing it.
const PLATE_EDGE_MARGIN_MM: f64 = 1e-3;

/// `after` (a new world transform of the model, from
/// [`turned_about_footprint`]) moved in the plate plane by the least amount
/// that keeps the model's shadow inside the plate, `plate_min` to
/// `plate_max`, when it was inside at `before`. Turning about the centre of
/// its footprint can reach past an edge although the model fits the plate.
/// On an axis where the model is wider than the plate, or when it was not
/// inside to begin with, it stays where the turn put it.
pub fn kept_on_plate(
    mesh: &IndexedMesh,
    before: &AffineTransform3D,
    after: &AffineTransform3D,
    plate_min: [f64; 2],
    plate_max: [f64; 2],
) -> AffineTransform3D {
    let (lo0, hi0) = world_bounds(mesh, before);
    let inside = (0..2).all(|k| lo0[k] >= plate_min[k] - 1e-6 && hi0[k] <= plate_max[k] + 1e-6);
    if !inside || !lo0[0].is_finite() {
        return *after;
    }
    let (lo, hi) = world_bounds(mesh, after);
    let mut shift = [0.0; 3];
    for k in 0..2 {
        let (min, max) = (
            plate_min[k] + PLATE_EDGE_MARGIN_MM,
            plate_max[k] - PLATE_EDGE_MARGIN_MM,
        );
        if hi[k] - lo[k] > max - min {
            continue;
        }
        if lo[k] < min {
            shift[k] = min - lo[k];
        } else if hi[k] > max {
            shift[k] = max - hi[k];
        }
    }
    if shift == [0.0; 3] {
        *after
    } else {
        AffineTransform3D::from_translation(shift).compose(after)
    }
}

/// The world transform of a model after it turns by `rotation` (row-major,
/// in world axes) about the centre of its footprint, keeping its lowest point
/// at the height it had: the lift stays, like the numeric transform does.
/// `world` is the model's current world transform (`parent_world()`
/// composed with its own).
pub fn turned_about_footprint(
    mesh: &IndexedMesh,
    world: &AffineTransform3D,
    rotation: &[[f64; 3]; 3],
) -> AffineTransform3D {
    let turn = AffineTransform3D::from_linear_and_translation(*rotation, [0.0; 3]);
    let turned = turn.compose(world);
    let (lo0, hi0) = world_bounds(mesh, world);
    let (lo1, hi1) = world_bounds(mesh, &turned);
    if !lo0[0].is_finite() {
        return *world;
    }
    let shift = [
        (lo0[0] + hi0[0] - lo1[0] - hi1[0]) / 2.0,
        (lo0[1] + hi0[1] - lo1[1] - hi1[1]) / 2.0,
        lo0[2] - lo1[2],
    ];
    AffineTransform3D::from_translation(shift).compose(&turned)
}
