//! three.js r181 `src/math/Quaternion.js` (MIT), the subset the support code calls.
//!
//! Also DragonFruit's `Quaternion` interface (`types.ts`): `{ x, y, z, w }`.

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
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    pub fn normalize(self) -> Self {
        let mut l = self.length();
        if l == 0.0 {
            Quaternion::IDENTITY
        } else {
            l = 1.0 / l;
            Quaternion::new(self.x * l, self.y * l, self.z * l, self.w * l)
        }
    }

    /// `setFromAxisAngle(axis, angle)`; `axis` is assumed normalized.
    /// http://www.euclideanspace.com/maths/geometry/rotations/conversions/angleToQuaternion/index.htm
    pub fn set_from_axis_angle(axis: Vec3, angle: f64) -> Self {
        let half_angle = angle / 2.0;
        let s = super::math_utils::js_sin(half_angle);
        Quaternion::new(
            axis.x * s,
            axis.y * s,
            axis.z * s,
            super::math_utils::js_cos(half_angle),
        )
    }

    /// Assumes direction vectors `v_from` and `v_to` are normalized.
    pub fn set_from_unit_vectors(v_from: Vec3, v_to: Vec3) -> Self {
        let mut r = v_from.dot(v_to) + 1.0;

        let q = if r < 1e-8 {
            // the epsilon value has been discussed in #31286
            // vFrom and vTo point in opposite directions
            r = 0.0;
            if v_from.x.abs() > v_from.z.abs() {
                Quaternion::new(-v_from.y, v_from.x, 0.0, r)
            } else {
                Quaternion::new(0.0, -v_from.z, v_from.y, r)
            }
        } else {
            // crossVectors( vFrom, vTo ); // inlined to avoid cyclic dependency on Vector3
            Quaternion::new(
                v_from.y * v_to.z - v_from.z * v_to.y,
                v_from.z * v_to.x - v_from.x * v_to.z,
                v_from.x * v_to.y - v_from.y * v_to.x,
                r,
            )
        };

        q.normalize()
    }

    /// `multiplyQuaternions(a, b)`.
    pub fn multiply_quaternions(a: Quaternion, b: Quaternion) -> Self {
        let (qax, qay, qaz, qaw) = (a.x, a.y, a.z, a.w);
        let (qbx, qby, qbz, qbw) = (b.x, b.y, b.z, b.w);
        Quaternion::new(
            qax * qbw + qaw * qbx + qay * qbz - qaz * qby,
            qay * qbw + qaw * qby + qaz * qbx - qax * qbz,
            qaz * qbw + qaw * qbz + qax * qby - qay * qbx,
            qaw * qbw - qax * qbx - qay * qby - qaz * qbz,
        )
    }

    /// `a.multiply(b)`.
    pub fn multiply(self, q: Quaternion) -> Self {
        Quaternion::multiply_quaternions(self, q)
    }
}
