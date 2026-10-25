//! three.js r181 `src/math/Matrix4.js` (MIT), the subset the support code calls.
//!
//! `elements` are column-major, as in three.js.

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
    #[rustfmt::skip]
    pub const IDENTITY: Matrix4 = Matrix4 {
        elements: [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// From column-major elements.
    pub const fn from_column_major(elements: [f64; 16]) -> Self {
        Matrix4 { elements }
    }

    /// `equals`: element-wise `===` (so NaN never equals).
    pub fn equals(&self, matrix: &Matrix4) -> bool {
        self.elements
            .iter()
            .zip(&matrix.elements)
            .all(|(a, b)| a == b)
    }

    pub fn determinant(&self) -> f64 {
        let te = &self.elements;

        let (n11, n12, n13, n14) = (te[0], te[4], te[8], te[12]);
        let (n21, n22, n23, n24) = (te[1], te[5], te[9], te[13]);
        let (n31, n32, n33, n34) = (te[2], te[6], te[10], te[14]);
        let (n41, n42, n43, n44) = (te[3], te[7], te[11], te[15]);

        n41 * (n14 * n23 * n32 - n13 * n24 * n32 - n14 * n22 * n33
            + n12 * n24 * n33
            + n13 * n22 * n34
            - n12 * n23 * n34)
            + n42
                * (n11 * n23 * n34 - n11 * n24 * n33 + n14 * n21 * n33 - n13 * n21 * n34
                    + n13 * n24 * n31
                    - n14 * n23 * n31)
            + n43
                * (n11 * n24 * n32 - n11 * n22 * n34 - n14 * n21 * n32
                    + n12 * n21 * n34
                    + n14 * n22 * n31
                    - n12 * n24 * n31)
            + n44
                * (-n13 * n22 * n31 - n11 * n23 * n32 + n11 * n22 * n33 + n13 * n21 * n32
                    - n12 * n21 * n33
                    + n12 * n23 * n31)
    }

    /// `invert()`: a singular matrix inverts to all zeros.
    /// based on http://www.euclideanspace.com/maths/algebra/matrix/functions/inverse/fourD/index.htm
    #[rustfmt::skip]
    pub fn invert(&self) -> Matrix4 {
        let te = &self.elements;
        let (n11, n21, n31, n41) = (te[0], te[1], te[2], te[3]);
        let (n12, n22, n32, n42) = (te[4], te[5], te[6], te[7]);
        let (n13, n23, n33, n43) = (te[8], te[9], te[10], te[11]);
        let (n14, n24, n34, n44) = (te[12], te[13], te[14], te[15]);

        let t11 = n23 * n34 * n42 - n24 * n33 * n42 + n24 * n32 * n43 - n22 * n34 * n43 - n23 * n32 * n44 + n22 * n33 * n44;
        let t12 = n14 * n33 * n42 - n13 * n34 * n42 - n14 * n32 * n43 + n12 * n34 * n43 + n13 * n32 * n44 - n12 * n33 * n44;
        let t13 = n13 * n24 * n42 - n14 * n23 * n42 + n14 * n22 * n43 - n12 * n24 * n43 - n13 * n22 * n44 + n12 * n23 * n44;
        let t14 = n14 * n23 * n32 - n13 * n24 * n32 - n14 * n22 * n33 + n12 * n24 * n33 + n13 * n22 * n34 - n12 * n23 * n34;

        let det = n11 * t11 + n21 * t12 + n31 * t13 + n41 * t14;

        if det == 0.0 {
            return Matrix4 { elements: [0.0; 16] };
        }

        let det_inv = 1.0 / det;
        let mut e = [0.0; 16];

        e[0] = t11 * det_inv;
        e[1] = (n24 * n33 * n41 - n23 * n34 * n41 - n24 * n31 * n43 + n21 * n34 * n43 + n23 * n31 * n44 - n21 * n33 * n44) * det_inv;
        e[2] = (n22 * n34 * n41 - n24 * n32 * n41 + n24 * n31 * n42 - n21 * n34 * n42 - n22 * n31 * n44 + n21 * n32 * n44) * det_inv;
        e[3] = (n23 * n32 * n41 - n22 * n33 * n41 - n23 * n31 * n42 + n21 * n33 * n42 + n22 * n31 * n43 - n21 * n32 * n43) * det_inv;

        e[4] = t12 * det_inv;
        e[5] = (n13 * n34 * n41 - n14 * n33 * n41 + n14 * n31 * n43 - n11 * n34 * n43 - n13 * n31 * n44 + n11 * n33 * n44) * det_inv;
        e[6] = (n14 * n32 * n41 - n12 * n34 * n41 - n14 * n31 * n42 + n11 * n34 * n42 + n12 * n31 * n44 - n11 * n32 * n44) * det_inv;
        e[7] = (n12 * n33 * n41 - n13 * n32 * n41 + n13 * n31 * n42 - n11 * n33 * n42 - n12 * n31 * n43 + n11 * n32 * n43) * det_inv;

        e[8] = t13 * det_inv;
        e[9] = (n14 * n23 * n41 - n13 * n24 * n41 - n14 * n21 * n43 + n11 * n24 * n43 + n13 * n21 * n44 - n11 * n23 * n44) * det_inv;
        e[10] = (n12 * n24 * n41 - n14 * n22 * n41 + n14 * n21 * n42 - n11 * n24 * n42 - n12 * n21 * n44 + n11 * n22 * n44) * det_inv;
        e[11] = (n13 * n22 * n41 - n12 * n23 * n41 - n13 * n21 * n42 + n11 * n23 * n42 + n12 * n21 * n43 - n11 * n22 * n43) * det_inv;

        e[12] = t14 * det_inv;
        e[13] = (n13 * n24 * n31 - n14 * n23 * n31 + n14 * n21 * n33 - n11 * n24 * n33 - n13 * n21 * n34 + n11 * n23 * n34) * det_inv;
        e[14] = (n14 * n22 * n31 - n12 * n24 * n31 - n14 * n21 * n32 + n11 * n24 * n32 + n12 * n21 * n34 - n11 * n22 * n34) * det_inv;
        e[15] = (n12 * n23 * n31 - n13 * n22 * n31 + n13 * n21 * n32 - n11 * n23 * n32 - n12 * n21 * n33 + n11 * n22 * n33) * det_inv;

        Matrix4 { elements: e }
    }

    /// The scale half of `decompose(position, quaternion, scale)`: column
    /// lengths, the first negated when the determinant is negative.
    pub fn decompose_scale(&self) -> Vec3 {
        let te = &self.elements;
        let mut sx = Vec3::new(te[0], te[1], te[2]).length();
        let sy = Vec3::new(te[4], te[5], te[6]).length();
        let sz = Vec3::new(te[8], te[9], te[10]).length();

        // if determine is negative, we need to invert one scale
        let det = self.determinant();
        if det < 0.0 {
            sx = -sx;
        }
        Vec3::new(sx, sy, sz)
    }

    /// `compose(position, quaternion, scale)`.
    pub fn compose(position: Vec3, quaternion: super::Quaternion, scale: Vec3) -> Matrix4 {
        let (x, y, z, w) = (quaternion.x, quaternion.y, quaternion.z, quaternion.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        let (sx, sy, sz) = (scale.x, scale.y, scale.z);
        Matrix4 {
            elements: [
                (1.0 - (yy + zz)) * sx,
                (xy + wz) * sx,
                (xz - wy) * sx,
                0.0,
                (xy - wz) * sy,
                (1.0 - (xx + zz)) * sy,
                (yz + wx) * sy,
                0.0,
                (xz + wy) * sz,
                (yz - wx) * sz,
                (1.0 - (xx + yy)) * sz,
                0.0,
                position.x,
                position.y,
                position.z,
                1.0,
            ],
        }
    }

    /// `makeRotationFromQuaternion(q)`: `compose(0, q, 1)`.
    pub fn make_rotation_from_quaternion(q: super::Quaternion) -> Matrix4 {
        Matrix4::compose(Vec3::ZERO, q, Vec3::new(1.0, 1.0, 1.0))
    }

    /// `setPosition(v)`.
    pub fn set_position(mut self, v: Vec3) -> Matrix4 {
        self.elements[12] = v.x;
        self.elements[13] = v.y;
        self.elements[14] = v.z;
        self
    }
}
