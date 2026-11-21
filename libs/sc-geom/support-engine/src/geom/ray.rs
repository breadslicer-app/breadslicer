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

    /// The closest points between the ray (from its origin on) and the
    /// segment `v0`-`v1`, and their squared distance (Ericson, Real-Time
    /// Collision Detection, 5.1.9, with the ray unbounded ahead).
    pub fn distance_sq_to_segment(self, v0: Vec3, v1: Vec3) -> RaySegmentDistance {
        let d1 = self.direction;
        let d2 = v1.sub(v0);
        let r = self.origin.sub(v0);
        let a = d1.dot(d1);
        let e = d2.dot(d2);
        let f = d2.dot(r);
        let c = d1.dot(r);
        let along_ray = |s: f64| if a > 0.0 { s.max(0.0) } else { 0.0 };
        let (s, t) = if e <= f64::EPSILON {
            (along_ray(-c / a), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s = if denom > 0.0 {
                ((b * f - c * e) / denom).max(0.0)
            } else {
                0.0
            };
            let t = (b * s + f) / e;
            if t < 0.0 {
                (along_ray(-c / a), 0.0)
            } else if t > 1.0 {
                (along_ray((b - c) / a), 1.0)
            } else {
                (s, t)
            }
        };
        let point_on_ray = self.origin.add_scaled_vector(d1, s);
        let point_on_segment = v0.add_scaled_vector(d2, t);
        RaySegmentDistance {
            sqr_dist: point_on_ray.distance_to_squared(point_on_segment),
            point_on_ray,
            point_on_segment,
        }
    }
}
