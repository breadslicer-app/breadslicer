/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! An axis-aligned box; the empty box has its corners inside out.

use super::matrix4::Matrix4;
use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Box3 {
    fn default() -> Self {
        Box3::EMPTY
    }
}

impl Box3 {
    pub const EMPTY: Box3 = Box3 {
        min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    };

    /// Whether the box holds no point (some axis has its max below its min).
    pub fn is_empty(&self) -> bool {
        self.max.x < self.min.x || self.max.y < self.min.y || self.max.z < self.min.z
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

    /// The box around the eight corners transformed by `matrix`; an empty box
    /// stays empty.
    pub fn apply_matrix4(&self, matrix: &Matrix4) -> Box3 {
        if self.is_empty() {
            return *self;
        }
        let (lo, hi) = (self.min, self.max);
        Box3::set_from_points((0..8).map(|corner| {
            Vec3::new(
                if corner & 4 == 0 { lo.x } else { hi.x },
                if corner & 2 == 0 { lo.y } else { hi.y },
                if corner & 1 == 0 { lo.z } else { hi.z },
            )
            .apply_matrix4(matrix)
        }))
    }
}
