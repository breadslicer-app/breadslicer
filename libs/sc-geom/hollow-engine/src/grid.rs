//! The voxel grid a hollow is worked out on, in the model's own frame.

use crate::mesh::{Aabb, Vec3};

/// Cubes of `voxel_mm` from `min`, `nx` by `ny` by `nz`, numbered along x,
/// then y, then z. Each cube is judged at its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub voxel_mm: f32,
    /// The outer corner of cube `(0, 0, 0)`.
    pub min: Vec3,
}

impl GridSpec {
    /// A grid over `bbox` with a cube of room on every side, so the cubes on
    /// its faces are always outside the model.
    pub fn for_bbox_with_voxel(bbox: &Aabb, voxel_mm: f32) -> Self {
        let pad = Vec3::new(voxel_mm, voxel_mm, voxel_mm);
        let min = bbox.min - pad;
        let size = bbox.max + pad - min;
        let cubes = |extent: f32| ((extent / voxel_mm).ceil() as usize).max(4);
        Self {
            nx: cubes(size.x),
            ny: cubes(size.y),
            nz: cubes(size.z),
            voxel_mm,
            min,
        }
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }

    #[inline]
    pub fn in_bounds(&self, x: isize, y: isize, z: isize) -> bool {
        (0..self.nx as isize).contains(&x)
            && (0..self.ny as isize).contains(&y)
            && (0..self.nz as isize).contains(&z)
    }

    #[inline]
    pub fn center_world(&self, x: usize, y: usize, z: usize) -> Vec3 {
        let at = |n: usize, from: f32| from + (n as f32 + 0.5) * self.voxel_mm;
        Vec3::new(at(x, self.min.x), at(y, self.min.y), at(z, self.min.z))
    }

    /// The `(x, y, z)` of cube number `index`.
    #[inline]
    pub fn coords(&self, index: usize) -> (usize, usize, usize) {
        let (x, rest) = (index % self.nx, index / self.nx);
        (x, rest % self.ny, rest / self.ny)
    }

    pub fn cell_count(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    /// The cube holding `p`, if the grid does.
    pub fn cell_at(&self, p: Vec3) -> Option<usize> {
        let at = |v: f32, from: f32| ((v - from) / self.voxel_mm).floor() as isize;
        let (x, y, z) = (
            at(p.x, self.min.x),
            at(p.y, self.min.y),
            at(p.z, self.min.z),
        );
        self.in_bounds(x, y, z)
            .then(|| self.idx(x as usize, y as usize, z as usize))
    }
}
