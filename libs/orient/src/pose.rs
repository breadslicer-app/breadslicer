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
    let bounds = |t: &AffineTransform3D| {
        let (l, o) = (t.linear(), t.translation());
        mesh.vertices().iter().fold(
            ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
            |(mut lo, mut hi), v| {
                let c = v.coords().map(f64::from);
                let p = mul(l, c);
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k] + o[k]);
                    hi[k] = hi[k].max(p[k] + o[k]);
                }
                (lo, hi)
            },
        )
    };
    let (lo0, hi0) = bounds(world);
    let (lo1, hi1) = bounds(&turned);
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
