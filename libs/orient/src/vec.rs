/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The few vector operations the search needs, on plain `[f64; 3]`.

pub(crate) type V3 = [f64; 3];

#[inline]
pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub(crate) fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
pub(crate) fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to length 1, or `None` for a zero or non-finite vector.
#[inline]
pub(crate) fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n > 0.0 && n.is_finite()).then(|| scale(a, 1.0 / n))
}

/// The angle between two unit vectors, in degrees.
#[inline]
pub(crate) fn angle_deg(a: V3, b: V3) -> f64 {
    dot(a, b).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Two unit vectors that complete `up` (a unit vector) to a right-handed
/// orthonormal basis.
pub(crate) fn basis(up: V3) -> (V3, V3) {
    let a = if up[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let e1 = unit(cross(a, up)).unwrap_or([1.0, 0.0, 0.0]);
    (e1, cross(up, e1))
}

/// Row-major 3×3 matrix times a vector.
#[inline]
pub(crate) fn mul(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
