//! three.js r181 `src/math/Vector2.js` (MIT), the subset the raft code calls.
//! Value semantics, as `Vector3`.

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

    /// `equals`: componentwise `===`.
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

    /// `divideScalar(this.length() || 1)`.
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

    /// `new Vector2().lerpVectors(v1, v2, alpha)`.
    #[inline]
    pub fn lerp_vectors(v1: Vector2, v2: Vector2, alpha: f64) -> Self {
        Vector2::new(v1.x + (v2.x - v1.x) * alpha, v1.y + (v2.y - v1.y) * alpha)
    }
}
