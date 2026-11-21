//! three.js `Matrix3`, the part the support code uses: the normal matrix.
//! `elements` are column-major, as in three.js.

use super::{Matrix4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3 {
    pub elements: [f64; 9],
}

impl Matrix3 {
    /// `setFromMatrix4`: the upper-left 3×3.
    pub fn from_matrix4(m: &Matrix4) -> Self {
        let me = &m.elements;
        Matrix3 {
            elements: [
                me[0], me[1], me[2], me[4], me[5], me[6], me[8], me[9], me[10],
            ],
        }
    }

    /// `invert()`; a singular matrix inverts to all zeros.
    pub fn invert(self) -> Self {
        let te = self.elements;
        let (n11, n21, n31) = (te[0], te[1], te[2]);
        let (n12, n22, n32) = (te[3], te[4], te[5]);
        let (n13, n23, n33) = (te[6], te[7], te[8]);
        let t11 = n33 * n22 - n32 * n23;
        let t12 = n32 * n13 - n33 * n12;
        let t13 = n23 * n12 - n22 * n13;
        let det = n11 * t11 + n21 * t12 + n31 * t13;
        if det == 0.0 {
            return Matrix3 { elements: [0.0; 9] };
        }
        let det_inv = 1.0 / det;
        Matrix3 {
            elements: [
                t11 * det_inv,
                (n31 * n23 - n33 * n21) * det_inv,
                (n32 * n21 - n31 * n22) * det_inv,
                t12 * det_inv,
                (n33 * n11 - n31 * n13) * det_inv,
                (n31 * n12 - n32 * n11) * det_inv,
                t13 * det_inv,
                (n21 * n13 - n23 * n11) * det_inv,
                (n22 * n11 - n21 * n12) * det_inv,
            ],
        }
    }

    pub fn transpose(self) -> Self {
        let mut m = self.elements;
        m.swap(1, 3);
        m.swap(2, 6);
        m.swap(5, 7);
        Matrix3 { elements: m }
    }

    /// `getNormalMatrix(m)`: the inverse transpose of the upper-left 3×3.
    pub fn normal_matrix(m: &Matrix4) -> Self {
        Matrix3::from_matrix4(m).invert().transpose()
    }
}

impl Vec3 {
    /// `applyMatrix3(m)`.
    pub fn apply_matrix3(self, m: &Matrix3) -> Self {
        let e = &m.elements;
        Vec3::new(
            e[0] * self.x + e[3] * self.y + e[6] * self.z,
            e[1] * self.x + e[4] * self.y + e[7] * self.z,
            e[2] * self.x + e[5] * self.y + e[8] * self.z,
        )
    }

    /// `applyNormalMatrix(m)`: `applyMatrix3(m).normalize()`.
    pub fn apply_normal_matrix(self, m: &Matrix3) -> Self {
        self.apply_matrix3(m).normalize()
    }
}
