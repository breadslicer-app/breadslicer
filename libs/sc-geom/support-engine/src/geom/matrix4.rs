/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A 4×4 affine transform, on `glam`. `elements` are column-major.

use glam::{DMat4, DQuat, DVec3};

use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix4 {
    pub elements: [f64; 16],
}

impl Default for Matrix4 {
    fn default() -> Self {
        Matrix4::IDENTITY
    }
}

impl Matrix4 {
    pub const IDENTITY: Matrix4 = Matrix4 {
        elements: DMat4::IDENTITY.to_cols_array(),
    };

    /// From column-major elements.
    pub const fn from_column_major(elements: [f64; 16]) -> Self {
        Matrix4 { elements }
    }

    #[inline]
    pub(crate) fn glam(&self) -> DMat4 {
        DMat4::from_cols_array(&self.elements)
    }

    #[inline]
    pub(crate) fn from_glam(m: DMat4) -> Self {
        Matrix4 {
            elements: m.to_cols_array(),
        }
    }

    /// Every element equal (so a NaN never is).
    pub fn equals(&self, matrix: &Matrix4) -> bool {
        self.elements == matrix.elements
    }

    pub fn determinant(&self) -> f64 {
        self.glam().determinant()
    }

    /// The inverse; a singular matrix inverts to all zeros.
    pub fn invert(&self) -> Matrix4 {
        let m = self.glam();
        if m.determinant() == 0.0 {
            return Matrix4 {
                elements: [0.0; 16],
            };
        }
        Matrix4::from_glam(m.inverse())
    }

    /// The scale of the transform: its columns' lengths, the first negated
    /// when it mirrors.
    pub fn decompose_scale(&self) -> Vec3 {
        let e = &self.elements;
        let length = |k: usize| DVec3::new(e[k], e[k + 1], e[k + 2]).length();
        let sx = if self.determinant() < 0.0 {
            -length(0)
        } else {
            length(0)
        };
        Vec3::new(sx, length(4), length(8))
    }

    /// Scale, then rotate, then move.
    pub fn compose(position: Vec3, quaternion: super::Quaternion, scale: Vec3) -> Matrix4 {
        Matrix4::from_glam(DMat4::from_scale_rotation_translation(
            scale.glam(),
            DQuat::from_xyzw(quaternion.x, quaternion.y, quaternion.z, quaternion.w),
            position.glam(),
        ))
    }

    pub fn make_rotation_from_quaternion(q: super::Quaternion) -> Matrix4 {
        Matrix4::compose(Vec3::ZERO, q, Vec3::new(1.0, 1.0, 1.0))
    }

    /// The same transform moved to `v`.
    pub fn set_position(mut self, v: Vec3) -> Matrix4 {
        self.elements[12] = v.x;
        self.elements[13] = v.y;
        self.elements[14] = v.z;
        self
    }

    pub fn make_translation(x: f64, y: f64, z: f64) -> Matrix4 {
        Matrix4::from_glam(DMat4::from_translation(DVec3::new(x, y, z)))
    }

    /// `self * b`: `b` applies first.
    pub fn multiply(&self, b: &Matrix4) -> Matrix4 {
        Matrix4::from_glam(self.glam() * b.glam())
    }
}
