/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A point or a direction in the plane, by value, as [`super::Vec3`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vector2 {
    pub x: f64,
    pub y: f64,
}

#[allow(clippy::should_implement_trait)]
impl Vector2 {
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Vector2 { x, y }
    }

    /// Both components equal.
    #[inline]
    pub fn equals(self, v: Vector2) -> bool {
        self.x == v.x && self.y == v.y
    }

    #[inline]
    pub fn add(self, v: Vector2) -> Self {
        Vector2::new(self.x + v.x, self.y + v.y)
    }

    #[inline]
    pub fn sub(self, v: Vector2) -> Self {
        Vector2::new(self.x - v.x, self.y - v.y)
    }

    #[inline]
    pub fn multiply_scalar(self, s: f64) -> Self {
        Vector2::new(self.x * s, self.y * s)
    }

    #[inline]
    pub fn dot(self, v: Vector2) -> f64 {
        self.x * v.x + self.y * v.y
    }

    #[inline]
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[inline]
    pub fn add_scaled_vector(self, v: Vector2, s: f64) -> Self {
        Vector2::new(self.x + v.x * s, self.y + v.y * s)
    }

    #[inline]
    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    /// Unit length; a zero vector is returned as it is.
    #[inline]
    pub fn normalize(self) -> Self {
        let l = self.length();
        self.multiply_scalar(1.0 / if l == 0.0 || l.is_nan() { 1.0 } else { l })
    }

    #[inline]
    pub fn distance_to_squared(self, v: Vector2) -> f64 {
        let (dx, dy) = (self.x - v.x, self.y - v.y);
        dx * dx + dy * dy
    }

    #[inline]
    pub fn distance_to(self, v: Vector2) -> f64 {
        self.distance_to_squared(v).sqrt()
    }

    /// The point `alpha` of the way from `v1` to `v2`.
    #[inline]
    pub fn lerp_vectors(v1: Vector2, v2: Vector2, alpha: f64) -> Self {
        Vector2::new(v1.x + (v2.x - v1.x) * alpha, v1.y + (v2.y - v1.y) * alpha)
    }
}
