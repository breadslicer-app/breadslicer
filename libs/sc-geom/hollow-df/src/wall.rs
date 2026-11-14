//! Exact wall distance near the surface.
//!
//! The session's chamfer distance over-measures by up to 12.8 % and starts
//! from voxels whose centres sit up to half a voxel diagonal off the surface,
//! so a wall cut from it is thicker than asked on average and thinner in
//! places. A wall is decided only near its own depth: there the engine
//! measures every voxel's exact distance to the source's triangles, with the
//! session's BVH, and keeps the chamfer for the voxels far from it.

use rayon::prelude::*;

use crate::control::{Cancelled, Control};
use crate::hollowing::HollowSession;

/// The chamfer's worst over-measure of a straight distance (12.8 %), rounded
/// up.
const CHAMFER_STRETCH: f32 = 1.13;
/// How far the centre of a voxel the surface passes through may be from the
/// surface: half its diagonal, in voxels.
const SURFACE_VOXEL_OFFSET: f32 = 0.866_025_4;
/// Room for chamfer paths the grid bends near the surface, in voxels.
const CHAMFER_SLACK: f32 = 2.0;

/// The largest chamfer distance a voxel within `band` voxels of the surface
/// can have. The voxel around its closest surface point is a surface voxel
/// at most `band + SURFACE_VOXEL_OFFSET` away, and the chamfer path to it is
/// at most 12.8 % longer than the straight line.
fn chamfer_limit(band: f32) -> f32 {
    CHAMFER_STRETCH * (band + SURFACE_VOXEL_OFFSET) + CHAMFER_SLACK
}

/// The wall distance of every voxel of a session, in voxels: exact for every
/// voxel within [`WallDistances::band`] of the surface, at least `band` for
/// the others.
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
    /// band of the same session, is reused where it is exact.
    pub(crate) fn measure(
        session: &HollowSession,
        band: f32,
        previous: Option<&WallDistances>,
        ctl: Control<'_>,
    ) -> Result<Self, Cancelled> {
        let grid = session.grid();
        let solid = session.solid_mask();
        let chamfer = session.chamfer_distances();
        let mesh = session.source_mesh();
        let bvh = session.source_bvh();
        let voxel = grid.voxel_mm;
        let limit = chamfer_limit(band);
        let reuse = previous.map(|p| (p.band, chamfer_limit(p.band)));
        let mut dist = match previous {
            Some(previous) => previous.dist.clone(),
            None => chamfer.to_vec(),
        };
        // Row by row: a few slices dense with surface would hold the other
        // cores up, a slice at a time (#451).
        let row = grid.nx;
        dist.par_chunks_mut(row)
            .enumerate()
            .try_for_each(|(r, cells)| {
                ctl.check()?;
                let (y, z) = (r % grid.ny, r / grid.ny);
                for (x, d) in cells.iter_mut().enumerate() {
                    let i = r * row + x;
                    let c = chamfer[i];
                    // The surface layer stays at 0: its centres lie on either
                    // side of the surface, and 0 only ever keeps material.
                    if !solid[i] || c == 0.0 || c > limit {
                        continue;
                    }
                    if let Some((old_band, old_limit)) = reuse
                        && c <= old_limit
                        && *d < old_band
                    {
                        continue;
                    }
                    // The chamfer bounds the distance from above: a path of
                    // `c` voxels to a surface voxel, then half its diagonal.
                    let cap = band.min(c + SURFACE_VOXEL_OFFSET);
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
