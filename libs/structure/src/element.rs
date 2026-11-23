/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! One beam: its stiffness in its own axes, and the turn to the world's.

use std::f64::consts::PI;

use crate::frame::Point;

pub(crate) type M12 = [[f64; 12]; 12];
pub(crate) type R3 = [[f64; 3]; 3];

/// Shear correction factor of a solid round section.
fn shear_factor(nu: f64) -> f64 {
    6.0 * (1.0 + nu) / (7.0 + 6.0 * nu)
}

/// A tapered round beam as one of even section: the area that stretches as
/// much, and the second moment that bends as much, at mid length.
pub(crate) fn even_section(d_a: f64, d_b: f64) -> (f64, f64) {
    let area = PI * d_a * d_b / 4.0;
    let inertia = PI * (d_a * d_b).powi(2) / 64.0;
    (area, inertia)
}

/// The stiffness of a round Timoshenko beam `l` long, of area `a` and second
/// moment `i`, in its own axes: x along it, y and z across. Degrees of
/// freedom at each end: u, v, w, θx, θy, θz.
pub(crate) fn local_k(l: f64, a: f64, i: f64, e: f64, nu: f64) -> M12 {
    let g = e / (2.0 * (1.0 + nu));
    let ea = e * a / l;
    let gj = g * 2.0 * i / l;
    let ei = e * i;
    // Shear deformation: phi = 12 E I / (k G A L²).
    let phi = 12.0 * ei / (shear_factor(nu) * g * a * l * l);
    let den = 1.0 + phi;
    let k1 = 12.0 * ei / (l.powi(3) * den);
    let k2 = 6.0 * ei / (l * l * den);
    let k3 = (4.0 + phi) * ei / (l * den);
    let k4 = (2.0 - phi) * ei / (l * den);
    let mut k = [[0.0; 12]; 12];
    let mut set = |r: usize, c: usize, v: f64| {
        k[r][c] = v;
        k[c][r] = v;
    };
    set(0, 0, ea);
    set(0, 6, -ea);
    set(6, 6, ea);
    set(3, 3, gj);
    set(3, 9, -gj);
    set(9, 9, gj);
    // Bending in the local xy plane (v = 1/7, θz = 5/11).
    set(1, 1, k1);
    set(1, 5, k2);
    set(1, 7, -k1);
    set(1, 11, k2);
    set(5, 5, k3);
    set(5, 7, -k2);
    set(5, 11, k4);
    set(7, 7, k1);
    set(7, 11, -k2);
    set(11, 11, k3);
    // Bending in the local xz plane (w = 2/8, θy = 4/10).
    set(2, 2, k1);
    set(2, 4, -k2);
    set(2, 8, -k1);
    set(2, 10, -k2);
    set(4, 4, k3);
    set(4, 8, k2);
    set(4, 10, k4);
    set(8, 8, k1);
    set(8, 10, k2);
    set(10, 10, k3);
    k
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: Point) -> Point {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}

/// The beam's axes in world coordinates, one per row: x from `a` to `b`.
/// The section is round, so any pair of axes across it will do.
pub(crate) fn rotation(a: Point, b: Point) -> R3 {
    let ex = unit(sub(b, a));
    let reference = if ex[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let ey = unit(cross(reference, ex));
    let ez = cross(ex, ey);
    [ex, ey, ez]
}

/// `T · v`, `T` the four blocks of `r` on the diagonal: world to local.
pub(crate) fn to_local(r: &R3, v: &[f64; 12]) -> [f64; 12] {
    let mut out = [0.0; 12];
    for blk in 0..4 {
        for i in 0..3 {
            out[blk * 3 + i] = (0..3).map(|j| r[i][j] * v[blk * 3 + j]).sum();
        }
    }
    out
}

/// `Tᵀ · v`: local to world.
pub(crate) fn to_global(r: &R3, v: &[f64; 12]) -> [f64; 12] {
    let mut out = [0.0; 12];
    for blk in 0..4 {
        for i in 0..3 {
            out[blk * 3 + i] = (0..3).map(|j| r[j][i] * v[blk * 3 + j]).sum();
        }
    }
    out
}

/// `Tᵀ · k · T`.
pub(crate) fn global_k(r: &R3, k: &M12) -> M12 {
    // k · T, row by row.
    let mut kt = [[0.0; 12]; 12];
    for (row, out) in k.iter().zip(kt.iter_mut()) {
        for blk in 0..4 {
            for j in 0..3 {
                out[blk * 3 + j] = (0..3).map(|i| row[blk * 3 + i] * r[i][j]).sum();
            }
        }
    }
    let mut out = [[0.0; 12]; 12];
    for c in 0..12 {
        let col: [f64; 12] = std::array::from_fn(|i| kt[i][c]);
        let g = to_global(r, &col);
        for i in 0..12 {
            out[i][c] = g[i];
        }
    }
    out
}
