//! three.js r181 `src/math/Ray.js` (MIT), the subset the support code calls.

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

    /// `at(t)`: `origin + direction * t`.
    #[inline]
    pub fn at(self, t: f64) -> Vec3 {
        self.origin.add_scaled_vector(self.direction, t)
    }

    /// `applyMatrix4(m)`: the origin as a point, the direction as a direction
    /// (renormalized).
    pub fn apply_matrix4(self, m: &super::Matrix4) -> Ray {
        Ray {
            origin: self.origin.apply_matrix4(m),
            direction: self.direction.transform_direction(m),
        }
    }

    /// `intersectTriangle(a, b, c, backfaceCulling)`: the hit point, if any.
    ///
    /// Compute the offset origin, edges, and normal.
    /// from https://github.com/pmjoniak/GeometricTools/blob/master/GTEngine/Include/Mathematics/GteIntrRay3Triangle3.h
    pub fn intersect_triangle(
        self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        backface_culling: bool,
    ) -> Option<Vec3> {
        let edge1 = Vec3::sub_vectors(b, a);
        let edge2 = Vec3::sub_vectors(c, a);
        let normal = Vec3::cross_vectors(edge1, edge2);

        // Solve Q + t*D = b1*E1 + b2*E2 (Q = kDiff, D = ray direction,
        // E1 = kEdge1, E2 = kEdge2, N = Cross(E1,E2)) by
        //   |Dot(D,N)|*b1 = sign(Dot(D,N))*Dot(D,Cross(Q,E2))
        //   |Dot(D,N)|*b2 = sign(Dot(D,N))*Dot(D,Cross(E1,Q))
        //   |Dot(D,N)|*t = -sign(Dot(D,N))*Dot(Q,N)
        let mut ddn = self.direction.dot(normal);
        let sign;
        if ddn > 0.0 {
            if backface_culling {
                return None;
            }
            sign = 1.0;
        } else if ddn < 0.0 {
            sign = -1.0;
            ddn = -ddn;
        } else {
            return None;
        }

        let diff = Vec3::sub_vectors(self.origin, a);
        let ddqxe2 = sign * self.direction.dot(Vec3::cross_vectors(diff, edge2));

        // b1 < 0, no intersection
        if ddqxe2 < 0.0 {
            return None;
        }

        let dde1xq = sign * self.direction.dot(edge1.cross(diff));

        // b2 < 0, no intersection
        if dde1xq < 0.0 {
            return None;
        }

        // b1+b2 > 1, no intersection
        if ddqxe2 + dde1xq > ddn {
            return None;
        }

        // Line intersects triangle, check if ray does.
        let qdn = -sign * diff.dot(normal);

        // t < 0, no intersection
        if qdn < 0.0 {
            return None;
        }

        // Ray intersects triangle.
        Some(self.at(qdn / ddn))
    }

    /// from https://github.com/pmjoniak/GeometricTools/blob/master/GTEngine/Include/Mathematics/GteDistRaySegment.h
    /// It returns the min distance between the ray and the segment defined by
    /// v0 and v1, with the closest point on each.
    pub fn distance_sq_to_segment(self, v0: Vec3, v1: Vec3) -> RaySegmentDistance {
        let seg_center = v0.add(v1).multiply_scalar(0.5);
        let seg_dir = v1.sub(v0).normalize();
        let diff = self.origin.sub(seg_center);

        let seg_extent = v0.distance_to(v1) * 0.5;
        let a01 = -self.direction.dot(seg_dir);
        let b0 = diff.dot(self.direction);
        let b1 = -diff.dot(seg_dir);
        let c = diff.length_sq();
        let det = (1.0 - a01 * a01).abs();
        let (mut s0, mut s1, sqr_dist);

        if det > 0.0 {
            // The ray and segment are not parallel.
            s0 = a01 * b1 - b0;
            s1 = a01 * b0 - b1;
            let ext_det = seg_extent * det;

            if s0 >= 0.0 {
                if s1 >= -ext_det {
                    if s1 <= ext_det {
                        // region 0
                        // Minimum at interior points of ray and segment.
                        let inv_det = 1.0 / det;
                        s0 *= inv_det;
                        s1 *= inv_det;
                        sqr_dist =
                            s0 * (s0 + a01 * s1 + 2.0 * b0) + s1 * (a01 * s0 + s1 + 2.0 * b1) + c;
                    } else {
                        // region 1
                        s1 = seg_extent;
                        s0 = 0.0_f64.max(-(a01 * s1 + b0));
                        sqr_dist = -s0 * s0 + s1 * (s1 + 2.0 * b1) + c;
                    }
                } else {
                    // region 5
                    s1 = -seg_extent;
                    s0 = 0.0_f64.max(-(a01 * s1 + b0));
                    sqr_dist = -s0 * s0 + s1 * (s1 + 2.0 * b1) + c;
                }
            } else if s1 <= -ext_det {
                // region 4
                s0 = 0.0_f64.max(-(-a01 * seg_extent + b0));
                s1 = if s0 > 0.0 {
                    -seg_extent
                } else {
                    (-seg_extent).max(-b1).min(seg_extent)
                };
                sqr_dist = -s0 * s0 + s1 * (s1 + 2.0 * b1) + c;
            } else if s1 <= ext_det {
                // region 3
                s0 = 0.0;
                s1 = (-seg_extent).max(-b1).min(seg_extent);
                sqr_dist = s1 * (s1 + 2.0 * b1) + c;
            } else {
                // region 2
                s0 = 0.0_f64.max(-(a01 * seg_extent + b0));
                s1 = if s0 > 0.0 {
                    seg_extent
                } else {
                    (-seg_extent).max(-b1).min(seg_extent)
                };
                sqr_dist = -s0 * s0 + s1 * (s1 + 2.0 * b1) + c;
            }
        } else {
            // Ray and segment are parallel.
            s1 = if a01 > 0.0 { -seg_extent } else { seg_extent };
            s0 = 0.0_f64.max(-(a01 * s1 + b0));
            sqr_dist = -s0 * s0 + s1 * (s1 + 2.0 * b1) + c;
        }

        RaySegmentDistance {
            sqr_dist,
            point_on_ray: self.origin.add_scaled_vector(self.direction, s0),
            point_on_segment: seg_center.add_scaled_vector(seg_dir, s1),
        }
    }
}
