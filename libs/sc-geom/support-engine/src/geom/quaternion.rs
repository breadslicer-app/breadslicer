/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A rotation as a quaternion `{ x, y, z, w }`, on `glam`.

use glam::DQuat;
use serde::{Deserialize, Serialize};

use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quaternion {
    fn default() -> Self {
        Quaternion::IDENTITY
    }
}

impl Quaternion {
    pub const IDENTITY: Quaternion = Quaternion {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Quaternion { x, y, z, w }
    }

    #[inline]
    fn glam(self) -> DQuat {
        DQuat::from_xyzw(self.x, self.y, self.z, self.w)
    }

    #[inline]
    fn from_glam(q: DQuat) -> Self {
        Quaternion::new(q.x, q.y, q.z, q.w)
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.glam().length()
    }

    /// Unit length; a zero quaternion becomes no rotation.
    pub fn normalize(self) -> Self {
        let l = self.length();
        if l == 0.0 {
            Quaternion::IDENTITY
        } else {
            Quaternion::from_glam(self.glam() / l)
        }
    }

    /// A turn of `angle` about the unit `axis`.
    pub fn set_from_axis_angle(axis: Vec3, angle: f64) -> Self {
        Quaternion::from_glam(DQuat::from_axis_angle(axis.glam(), angle))
    }

    /// The shortest turn from unit `v_from` to unit `v_to`; half a turn about
    /// an axis square to `v_from` when they are opposite.
    pub fn set_from_unit_vectors(v_from: Vec3, v_to: Vec3) -> Self {
        Quaternion::from_glam(DQuat::from_rotation_arc(v_from.glam(), v_to.glam()))
    }

    /// `a * b`: `b` turns first.
    pub fn multiply_quaternions(a: Quaternion, b: Quaternion) -> Self {
        Quaternion::from_glam(a.glam() * b.glam())
    }

    pub fn multiply(self, q: Quaternion) -> Self {
        Quaternion::multiply_quaternions(self, q)
    }
}
