//! Exact wall distance near the surface.
//!
//! A wall is decided only near its own depth: there every inside cube's
//! exact distance to the source's triangles is measured with the model's
//! tree. The cubes the seed distance puts surely farther than that are only
//! known to be farther.

use rayon::prelude::*;

use crate::control::{Cancelled, Control};
use crate::volume::{SEED_SLACK, Volume};

/// The wall distance of every cube of a volume, in voxels: exact for every
/// inside cube within [`WallDistances::band`] of the surface, at least
/// `band` for the others, 0 outside.
pub(crate) struct WallDistances {
    band: f32,
    dist: Vec<f32>,
}

impl std::fmt::Debug for WallDistances {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WallDistances")
            .field("band", &self.band)
            .field("voxels", &self.dist.len())
            .finish()
    }
}

impl WallDistances {
    /// How far from the surface, in voxels, the distances are exact.
    pub(crate) fn band(&self) -> f32 {
        self.band
    }

    /// Measures the distances out to `band` voxels. `previous`, a narrower
    /// band of the same volume, is kept where it is exact.
    pub(crate) fn measure(
        volume: &Volume,
        band: f32,
        previous: Option<&WallDistances>,
        ctl: Control<'_>,
    ) -> Result<Self, Cancelled> {
        let grid = volume.grid();
        let solid = volume.solid_mask();
        let near = volume.near();
        let (mesh, bvh) = (volume.source_mesh(), volume.source_bvh());
        let voxel = grid.voxel_mm;
        let old_band = previous.map(|p| p.band);
        let mut dist = match previous {
            Some(previous) => previous.dist.clone(),
            None => vec![0.0; grid.cell_count()],
        };
        // A row at a time: a few slices thick with surface would hold the
        // other cores up, a slice at a time (#451).
        dist.par_chunks_mut(grid.nx)
            .enumerate()
            .try_for_each(|(row, cells)| {
                ctl.check()?;
                let (y, z) = (row % grid.ny, row / grid.ny);
                for (x, d) in cells.iter_mut().enumerate() {
                    let i = row * grid.nx + x;
                    if !solid[i] || old_band.is_some_and(|old| *d < old) {
                        continue;
                    }
                    if near[i] - SEED_SLACK > band {
                        *d = band;
                        continue;
                    }
                    let cap = band.min(near[i] + SEED_SLACK);
                    let p = grid.center_world(x, y, z);
                    *d = bvh
                        .nearest_distance(mesh, p, cap * voxel * 1.0001)
                        .map_or(cap, |mm| (mm / voxel).min(cap));
                }
                Ok(())
            })?;
        Ok(Self { band, dist })
    }

    /// The distances a wall reading out to `band` voxels uses: exact up to
    /// `band`, `band` beyond. The same whatever wider band was measured, so a
    /// hollow never depends on the walls asked for before it.
    pub(crate) fn clamped(&self, band: f32) -> Vec<f32> {
        debug_assert!(band <= self.band);
        self.dist.par_iter().map(|&d| d.min(band)).collect()
    }
}

#[cfg(test)]
#[path = "wall_tests.rs"]
mod tests;
