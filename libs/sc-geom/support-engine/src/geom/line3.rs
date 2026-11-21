/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A segment from `start` to `end`.

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
        self.end.sub(self.start)
    }

    #[inline]
    pub fn distance_sq(self) -> f64 {
        self.start.distance_to_squared(self.end)
    }

    #[inline]
    pub fn distance(self) -> f64 {
        self.start.distance_to(self.end)
    }

    /// The point `t` of the way from `start` to `end`.
    #[inline]
    pub fn at(self, t: f64) -> Vec3 {
        self.start.add_scaled_vector(self.delta(), t)
    }

    /// Where along the line (0 at `start`, 1 at `end`) `point` projects,
    /// kept on the segment with `clamp_to_line`.
    pub fn closest_point_to_point_parameter(self, point: Vec3, clamp_to_line: bool) -> f64 {
        let delta = self.delta();
        let t = delta.dot(point.sub(self.start)) / delta.dot(delta);
        if clamp_to_line { clamp(t, 0.0, 1.0) } else { t }
    }

    pub fn closest_point_to_point(self, point: Vec3, clamp_to_line: bool) -> Vec3 {
        self.at(self.closest_point_to_point_parameter(point, clamp_to_line))
    }
}
