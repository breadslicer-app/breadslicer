/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A 3×3 matrix, on `glam`: the normal matrix of a transform. `elements` are
//! column-major.

use glam::DMat3;

use super::{Matrix4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3 {
    pub elements: [f64; 9],
}

impl Matrix3 {
    #[inline]
    fn glam(self) -> DMat3 {
        DMat3::from_cols_array(&self.elements)
    }

    #[inline]
    fn from_glam(m: DMat3) -> Self {
        Matrix3 {
            elements: m.to_cols_array(),
        }
    }

    /// The upper-left 3×3 of `m`.
    pub fn from_matrix4(m: &Matrix4) -> Self {
        Matrix3::from_glam(DMat3::from_mat4(m.glam()))
    }

    /// The inverse; a singular matrix inverts to all zeros.
    pub fn invert(self) -> Self {
        let m = self.glam();
        if m.determinant() == 0.0 {
            return Matrix3 { elements: [0.0; 9] };
        }
        Matrix3::from_glam(m.inverse())
    }

    pub fn transpose(self) -> Self {
        Matrix3::from_glam(self.glam().transpose())
    }

    /// The inverse transpose of the upper-left 3×3: what turns normals.
    pub fn normal_matrix(m: &Matrix4) -> Self {
        Matrix3::from_matrix4(m).invert().transpose()
    }
}

impl Vec3 {
    pub fn apply_matrix3(self, m: &Matrix3) -> Self {
        Vec3::from_glam(m.glam() * self.glam())
    }

    /// A normal turned by a normal matrix, unit length.
    pub fn apply_normal_matrix(self, m: &Matrix3) -> Self {
        self.apply_matrix3(m).normalize()
    }
}
