//! three.js r181 `src/math/Box3.js` (MIT), the subset the support code calls.

use super::matrix4::Matrix4;
use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Box3 {
    /// `new Box3()` is empty.
    fn default() -> Self {
        Box3::EMPTY
    }
}

impl Box3 {
    pub const EMPTY: Box3 = Box3 {
        min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    };

    /// this is a more robust check for empty than ( volume <= 0 ) because volume
    /// can get positive with two negative axes
    pub fn is_empty(&self) -> bool {
        (self.max.x < self.min.x) || (self.max.y < self.min.y) || (self.max.z < self.min.z)
    }

    pub fn expand_by_point(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    pub fn set_from_points(points: impl IntoIterator<Item = Vec3>) -> Box3 {
        let mut b = Box3::EMPTY;
        for p in points {
            b.expand_by_point(p);
        }
        b
    }

    /// transform of empty box is an empty box.
    pub fn apply_matrix4(&self, matrix: &Matrix4) -> Box3 {
        if self.is_empty() {
            return *self;
        }
        let (mn, mx) = (self.min, self.max);
        // NOTE: I am using a binary pattern to specify all 2^3 combinations below
        Box3::set_from_points([
            Vec3::new(mn.x, mn.y, mn.z).apply_matrix4(matrix), // 000
            Vec3::new(mn.x, mn.y, mx.z).apply_matrix4(matrix), // 001
            Vec3::new(mn.x, mx.y, mn.z).apply_matrix4(matrix), // 010
            Vec3::new(mn.x, mx.y, mx.z).apply_matrix4(matrix), // 011
            Vec3::new(mx.x, mn.y, mn.z).apply_matrix4(matrix), // 100
            Vec3::new(mx.x, mn.y, mx.z).apply_matrix4(matrix), // 101
            Vec3::new(mx.x, mx.y, mn.z).apply_matrix4(matrix), // 110
            Vec3::new(mx.x, mx.y, mx.z).apply_matrix4(matrix), // 111
        ])
    }
}
