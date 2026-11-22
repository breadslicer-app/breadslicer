/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A ray: where it meets a triangle, and how close it passes a segment.

use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// The closest pair between a ray and a segment, with their squared distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaySegmentDistance {
    pub sqr_dist: f64,
    pub point_on_ray: Vec3,
    pub point_on_segment: Vec3,
}

impl Default for Ray {
    fn default() -> Self {
        Ray {
            origin: Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, -1.0),
        }
    }
}

impl Ray {
    #[inline]
    pub const fn new(origin: Vec3, direction: Vec3) -> Self {
        Ray { origin, direction }
    }

    /// `origin + direction * t`.
    #[inline]
    pub fn at(self, t: f64) -> Vec3 {
        self.origin.add_scaled_vector(self.direction, t)
    }

    /// The ray transformed by `m`: its origin as a point, its direction as a
    /// unit direction.
    pub fn apply_matrix4(self, m: &super::Matrix4) -> Ray {
        Ray {
            origin: self.origin.apply_matrix4(m),
            direction: self.direction.transform_direction(m),
        }
    }

    /// Where the ray meets triangle `a, b, c` (Moller-Trumbore), if it does,
    /// on or ahead of its origin. With `backface_culling`, a triangle seen
    /// from its back (`a, b, c` clockwise from the ray) is not met.
    pub fn intersect_triangle(
        self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        backface_culling: bool,
    ) -> Option<Vec3> {
        let e1 = b.sub(a);
        let e2 = c.sub(a);
        let p = self.direction.cross(e2);
        let det = e1.dot(p);
        // Seen from its back the determinant is negative: the ray runs with
        // the normal e1 x e2.
        if det == 0.0 || (backface_culling && det < 0.0) {
            return None;
        }
        let inv = 1.0 / det;
        let s = self.origin.sub(a);
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = self.direction.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(q) * inv;
        (t >= 0.0).then(|| self.at(t))
    }
}
