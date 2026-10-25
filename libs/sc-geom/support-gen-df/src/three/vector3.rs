//! three.js r181 `src/math/Vector3.js` (MIT), the subset the support code calls.
//!
//! Value semantics: every method takes `self` and returns the result, so
//! three's `a.clone().add(b)` reads `a.add(b)`. The arithmetic is written in
//! three's own order (`normalize` multiplies by `1 / length`, `lerp` adds the
//! scaled delta) so results match DragonFruit bit for bit where IEEE allows.
//!
//! This is also DragonFruit's plain `Vec3` interface (`types.ts`): a bare
//! `{ x, y, z }` record on the wire.

use serde::{Deserialize, Serialize};

use super::math_utils::clamp;
use super::quaternion::Quaternion;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// DragonFruit spells the same record `Vec3` (data) and `THREE.Vector3` (math).
pub type Vector3 = Vec3;

// three.js names: `add`/`sub` keep the transliterated call sites readable
// against DragonFruit, and take `self` by value like the operators would.
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
    pub fn add(self, v: Vec3) -> Self {
        Vec3::new(self.x + v.x, self.y + v.y, self.z + v.z)
    }

    #[inline]
    pub fn add_scaled_vector(self, v: Vec3, s: f64) -> Self {
        Vec3::new(self.x + v.x * s, self.y + v.y * s, self.z + v.z * s)
    }

    #[inline]
    pub fn sub(self, v: Vec3) -> Self {
        Vec3::new(self.x - v.x, self.y - v.y, self.z - v.z)
    }

    /// `new Vector3().subVectors(a, b)`.
    #[inline]
    pub fn sub_vectors(a: Vec3, b: Vec3) -> Self {
        Vec3::new(a.x - b.x, a.y - b.y, a.z - b.z)
    }

    #[inline]
    pub fn multiply_scalar(self, scalar: f64) -> Self {
        Vec3::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }

    #[inline]
    pub fn divide_scalar(self, scalar: f64) -> Self {
        self.multiply_scalar(1.0 / scalar)
    }

    #[inline]
    pub fn negate(self) -> Self {
        Vec3::new(-self.x, -self.y, -self.z)
    }

    #[inline]
    pub fn dot(self, v: Vec3) -> f64 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    #[inline]
    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[inline]
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// `divideScalar(this.length() || 1)`: a zero vector stays zero.
    #[inline]
    pub fn normalize(self) -> Self {
        let length = self.length();
        self.divide_scalar(if length == 0.0 || length.is_nan() {
            1.0
        } else {
            length
        })
    }

    #[inline]
    pub fn set_length(self, length: f64) -> Self {
        self.normalize().multiply_scalar(length)
    }

    #[inline]
    pub fn lerp(self, v: Vec3, alpha: f64) -> Self {
        Vec3::new(
            self.x + (v.x - self.x) * alpha,
            self.y + (v.y - self.y) * alpha,
            self.z + (v.z - self.z) * alpha,
        )
    }

    /// `crossVectors(this, v)`.
    #[inline]
    pub fn cross(self, v: Vec3) -> Self {
        Vec3::cross_vectors(self, v)
    }

    #[inline]
    pub fn cross_vectors(a: Vec3, b: Vec3) -> Self {
        let (ax, ay, az) = (a.x, a.y, a.z);
        let (bx, by, bz) = (b.x, b.y, b.z);
        Vec3::new(ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx)
    }

    pub fn project_on_vector(self, v: Vec3) -> Self {
        let denominator = v.length_sq();
        if denominator == 0.0 {
            return Vec3::ZERO;
        }
        let scalar = v.dot(self) / denominator;
        v.multiply_scalar(scalar)
    }

    pub fn angle_to(self, v: Vec3) -> f64 {
        let denominator = (self.length_sq() * v.length_sq()).sqrt();
        if denominator == 0.0 {
            return std::f64::consts::PI / 2.0;
        }
        let theta = self.dot(v) / denominator;
        clamp(theta, -1.0, 1.0).acos()
    }

    #[inline]
    pub fn distance_to(self, v: Vec3) -> f64 {
        self.distance_to_squared(v).sqrt()
    }

    #[inline]
    pub fn distance_to_squared(self, v: Vec3) -> f64 {
        let (dx, dy, dz) = (self.x - v.x, self.y - v.y, self.z - v.z);
        dx * dx + dy * dy + dz * dz
    }

    /// Quaternion `q` is assumed to have unit length.
    pub fn apply_quaternion(self, q: Quaternion) -> Self {
        let (vx, vy, vz) = (self.x, self.y, self.z);
        let (qx, qy, qz, qw) = (q.x, q.y, q.z, q.w);

        // t = 2 * cross( q.xyz, v );
        let tx = 2.0 * (qy * vz - qz * vy);
        let ty = 2.0 * (qz * vx - qx * vz);
        let tz = 2.0 * (qx * vy - qy * vx);

        // v + q.w * t + cross( q.xyz, t );
        Vec3::new(
            vx + qw * tx + qy * tz - qz * ty,
            vy + qw * ty + qz * tx - qx * tz,
            vz + qw * tz + qx * ty - qy * tx,
        )
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}
