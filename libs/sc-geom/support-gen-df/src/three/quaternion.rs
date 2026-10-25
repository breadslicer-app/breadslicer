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
}
