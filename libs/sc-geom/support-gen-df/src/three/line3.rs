//! three.js r181 `src/math/Line3.js` (MIT), the subset the support code calls.

use super::math_utils::clamp;
use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Line3 {
    pub start: Vec3,
    pub end: Vec3,
}

impl Line3 {
    #[inline]
    pub const fn new(start: Vec3, end: Vec3) -> Self {
        Line3 { start, end }
    }

    #[inline]
    pub fn delta(self) -> Vec3 {
        Vec3::sub_vectors(self.end, self.start)
    }

    #[inline]
    pub fn distance_sq(self) -> f64 {
        self.start.distance_to_squared(self.end)
    }

    #[inline]
    pub fn distance(self) -> f64 {
        self.start.distance_to(self.end)
    }

    #[inline]
    pub fn at(self, t: f64) -> Vec3 {
        self.delta().multiply_scalar(t).add(self.start)
    }

    pub fn closest_point_to_point_parameter(self, point: Vec3, clamp_to_line: bool) -> f64 {
        let start_p = Vec3::sub_vectors(point, self.start);
        let start_end = Vec3::sub_vectors(self.end, self.start);

        let start_end2 = start_end.dot(start_end);
        let start_end_start_p = start_end.dot(start_p);

        let mut t = start_end_start_p / start_end2;
        if clamp_to_line {
            t = clamp(t, 0.0, 1.0);
        }
        t
    }

    pub fn closest_point_to_point(self, point: Vec3, clamp_to_line: bool) -> Vec3 {
        let t = self.closest_point_to_point_parameter(point, clamp_to_line);
        self.delta().multiply_scalar(t).add(self.start)
    }
}
