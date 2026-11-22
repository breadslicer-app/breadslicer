/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A triangle: its normal and the point of it closest to a point.

use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

impl Triangle {
    pub const fn new(a: Vec3, b: Vec3, c: Vec3) -> Self {
        Triangle { a, b, c }
    }

    /// The unit normal of `a, b, c` (counter-clockwise is the front), zero
    /// for a degenerate triangle.
    pub fn get_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
        let n = c.sub(b).cross(a.sub(b));
        let length_sq = n.length_sq();
        if length_sq > 0.0 {
            n.multiply_scalar(1.0 / length_sq.sqrt())
        } else {
            Vec3::ZERO
        }
    }

    /// The point of the triangle closest to `p`: which vertex, edge or face
    /// region `p` falls in, from its barycentric parameters (Ericson,
    /// Real-Time Collision Detection, 5.1.5).
    pub fn closest_point_to_point(&self, p: Vec3) -> Vec3 {
        let (a, b, c) = (self.a, self.b, self.c);
        let ab = b.sub(a);
        let ac = c.sub(a);
        let ap = p.sub(a);
        let (d1, d2) = (ab.dot(ap), ac.dot(ap));
        if d1 <= 0.0 && d2 <= 0.0 {
            return a;
        }
        let bp = p.sub(b);
        let (d3, d4) = (ab.dot(bp), ac.dot(bp));
        if d3 >= 0.0 && d4 <= d3 {
            return b;
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            return a.add_scaled_vector(ab, d1 / (d1 - d3));
        }
        let cp = p.sub(c);
        let (d5, d6) = (ab.dot(cp), ac.dot(cp));
        if d6 >= 0.0 && d5 <= d6 {
            return c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            return a.add_scaled_vector(ac, d2 / (d2 - d6));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            return b.add_scaled_vector(c.sub(b), w);
        }
        let denom = 1.0 / (va + vb + vc);
        a.add_scaled_vector(ab, vb * denom)
            .add_scaled_vector(ac, vc * denom)
    }
}
