/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A point or a direction in millimetres, by value: every method returns a
//! new vector. Its `{ x, y, z }` form is also how support documents store it.

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

use super::math_utils::{clamp, js_max, js_min};
use super::quaternion::Quaternion;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// The same vector, named for the code that reads it as geometry.
pub type Vector3 = Vec3;

#[allow(clippy::should_implement_trait)]
impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    #[inline]
    pub(crate) fn glam(self) -> DVec3 {
        DVec3::new(self.x, self.y, self.z)
    }

    #[inline]
    pub(crate) fn from_glam(v: DVec3) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }

    #[inline]
    pub fn add(self, v: Vec3) -> Self {
        Vec3::from_glam(self.glam() + v.glam())
    }

    #[inline]
    pub fn add_scaled_vector(self, v: Vec3, s: f64) -> Self {
        Vec3::from_glam(self.glam() + v.glam() * s)
    }

    #[inline]
    pub fn sub(self, v: Vec3) -> Self {
        Vec3::from_glam(self.glam() - v.glam())
    }

    /// `a - b`.
    #[inline]
    pub fn sub_vectors(a: Vec3, b: Vec3) -> Self {
        a.sub(b)
    }

    #[inline]
    pub fn multiply_scalar(self, scalar: f64) -> Self {
        Vec3::from_glam(self.glam() * scalar)
    }

    #[inline]
    pub fn divide_scalar(self, scalar: f64) -> Self {
        self.multiply_scalar(1.0 / scalar)
    }

    #[inline]
    pub fn negate(self) -> Self {
        Vec3::from_glam(-self.glam())
    }

    #[inline]
    pub fn dot(self, v: Vec3) -> f64 {
        self.glam().dot(v.glam())
    }

    #[inline]
    pub fn length_sq(self) -> f64 {
        self.glam().length_squared()
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.glam().length()
    }

    /// Unit length; a zero (or NaN-long) vector is returned as it is.
    #[inline]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length == 0.0 || length.is_nan() {
            self
        } else {
            self.divide_scalar(length)
        }
    }

    #[inline]
    pub fn lerp(self, v: Vec3, alpha: f64) -> Self {
        Vec3::from_glam(self.glam() + (v.glam() - self.glam()) * alpha)
    }

    #[inline]
    pub fn cross(self, v: Vec3) -> Self {
        Vec3::from_glam(self.glam().cross(v.glam()))
    }

    #[inline]
    pub fn cross_vectors(a: Vec3, b: Vec3) -> Self {
        a.cross(b)
    }

    /// The angle to `v` in radians; a right angle when either is zero.
    pub fn angle_to(self, v: Vec3) -> f64 {
        let denominator = (self.length_sq() * v.length_sq()).sqrt();
        if denominator == 0.0 {
            return std::f64::consts::FRAC_PI_2;
        }
        clamp(self.dot(v) / denominator, -1.0, 1.0).acos()
    }

    #[inline]
    pub fn distance_to(self, v: Vec3) -> f64 {
        self.glam().distance(v.glam())
    }

    #[inline]
    pub fn distance_to_squared(self, v: Vec3) -> f64 {
        self.glam().distance_squared(v.glam())
    }

    /// Turned by the unit quaternion `q`.
    pub fn apply_quaternion(self, q: Quaternion) -> Self {
        Vec3::from_glam(DQuat::from_xyzw(q.x, q.y, q.z, q.w) * self.glam())
    }

    /// Transformed by `m` as a point, with the perspective divide.
    pub fn apply_matrix4(self, m: &super::Matrix4) -> Self {
        Vec3::from_glam(m.glam().project_point3(self.glam()))
    }

    /// Componentwise between `min` and `max`.
    #[inline]
    pub fn clamp(self, min: Vec3, max: Vec3) -> Self {
        Vec3::new(
            clamp(self.x, min.x, max.x),
            clamp(self.y, min.y, max.y),
            clamp(self.z, min.z, max.z),
        )
    }

    /// Componentwise smaller; a NaN component wins.
    #[inline]
    pub fn min(self, v: Vec3) -> Self {
        Vec3::new(
            js_min(self.x, v.x),
            js_min(self.y, v.y),
            js_min(self.z, v.z),
        )
    }

    /// Componentwise larger; a NaN component wins.
    #[inline]
    pub fn max(self, v: Vec3) -> Self {
        Vec3::new(
            js_max(self.x, v.x),
            js_max(self.y, v.y),
            js_max(self.z, v.z),
        )
    }

    /// Componentwise product.
    #[inline]
    pub fn multiply(self, v: Vec3) -> Self {
        Vec3::from_glam(self.glam() * v.glam())
    }

    /// Turned (and scaled) by the linear part of `m`, then unit length.
    pub fn transform_direction(self, m: &super::Matrix4) -> Self {
        Vec3::from_glam(m.glam().transform_vector3(self.glam())).normalize()
    }

    /// The scale of `m`: its first three columns' lengths.
    pub fn from_matrix_scale(m: &super::Matrix4) -> Self {
        let e = &m.elements;
        let length = |k: usize| DVec3::new(e[k], e[k + 1], e[k + 2]).length();
        Vec3::new(length(0), length(4), length(8))
    }

    /// Turned by `angle` about the unit `axis`.
    pub fn apply_axis_angle(self, axis: Vec3, angle: f64) -> Self {
        self.apply_quaternion(Quaternion::set_from_axis_angle(axis, angle))
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.glam().is_finite()
    }
}
