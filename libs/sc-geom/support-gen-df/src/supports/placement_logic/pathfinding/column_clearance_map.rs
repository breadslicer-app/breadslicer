//! DragonFruit `src/supports/PlacementLogic/Pathfinding/ColumnClearanceMap.ts`.
//!
//! Column clearance map: one scalar per XY cell - the lowest Z that a ball of
//! `clearance` around a *mesh vertex* reaches - which answers the router's
//! column query without marching the distance field.
//!
//! Why vertices. Every rasterized version of this had to bound how far a cell
//! could sit from the surface it stood for, and that bound is what killed them: a
//! triangle that only clips a cell charges its lowest vertex to a cell up to a
//! triangle-width away, which on a coarse part is hundreds of times the error the
//! verdict can absorb. A mesh vertex has no such error - it *is* on the solid -
//! so a ball of `clearance` around it is provably inside the blocked set. That
//! makes `blocked` sound by construction, with no conservatism and no error
//! budget beyond the query's own snap to a cell centre.
//!
//! Why a scalar is enough. The router's columns all run from a walk point down
//! to `rootTopZ`, and `rootTopZ` is below the model. So a column is blocked
//! exactly when the blocked set at that XY has an element in `[rootTopZ, zTop]`,
//! and with the set's minimum `lo` that is `lo <= zTop` - provided `lo` is a real
//! blocked Z. The whole Z-interval structure therefore collapses to its minimum,
//! which is what makes the build a single min-push instead of per-cell lists.
//!
//! What it cannot say: `clear`. Proving nothing is within clearance needs the map
//! to be *complete*, and vertices are not - a point can sit near a triangle's
//! interior, far from all three corners. So `clear` falls through to the march.
//!
//! `buildMs` (wall-clock) is not recorded: it is a report, never read.

use rayon::prelude::*;

use crate::three::math_utils::{js_max, js_min};
use crate::three::{Matrix4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnVerdict {
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnClearanceMapStats {
    pub cell_mm: f64,
    pub width: usize,
    pub height: usize,
    pub cells: usize,
    /// Radius the balls were drawn at: `clearance` minus the query's snap.
    pub radius_mm: f64,
    pub vertex_count: usize,
    pub bytes: usize,
}

const MAX_CELLS: f64 = 12_000_000.0;
const SLOP_MM: f64 = 0.02;
const NO_SOLID: f32 = f32::INFINITY;

#[derive(Debug, Clone)]
pub struct ColumnClearanceMap {
    pub stats: ColumnClearanceMapStats,
    cell: f64,
    ox: f64,
    oy: f64,
    w: usize,
    h: usize,
    lo: Vec<f32>,
}

impl ColumnClearanceMap {
    /// Build from a mesh's position attribute under its world matrix. Returns
    /// `None` when the grid would be unreasonably large.
    ///
    /// Each vertex is an exact point of the solid, so the ball drawn around it is
    /// a set of points that are all within `clearance` of the model. The only
    /// error left is the query: it snaps to a cell centre, so the radius is
    /// shrunk by half a cell diagonal (the worst case distance from a cell centre
    /// to a point inside it) to keep the claim true for the point actually asked
    /// about.
    pub fn build(
        positions: &[[f32; 3]],
        matrix_world: &Matrix4,
        clearance_mm: f64,
        cell_mm: f64,
    ) -> Option<Self> {
        // World positions once, in parallel: the bounds and every band below
        // read them.
        let world: Vec<Vec3> = positions
            .par_iter()
            .map(|p| {
                Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
                    .apply_matrix4(matrix_world)
            })
            .collect();

        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for v in &world {
            if v.x < min_x {
                min_x = v.x;
            }
            if v.y < min_y {
                min_y = v.y;
            }
            if v.x > max_x {
                max_x = v.x;
            }
            if v.y > max_y {
                max_y = v.y;
            }
        }
        let w = ((max_x - min_x) / cell_mm).ceil() + 2.0;
        let h = ((max_y - min_y) / cell_mm).ceil() + 2.0;
        if !(w * h).is_finite() || w * h > MAX_CELLS {
            return None;
        }
        let (w, h) = (w as usize, h as usize);
        let ox = min_x - cell_mm;
        let oy = min_y - cell_mm;

        let snap_mm = cell_mm * std::f64::consts::FRAC_1_SQRT_2 + SLOP_MM;
        let radius = js_max(0.01, clearance_mm - snap_mm);
        let r2 = radius * radius;
        let rc = (radius / cell_mm).ceil() as i64;

        // Vertices bucketed by the row they fall in (a counting sort), so a
        // band of rows finds the vertices that reach it without a full pass.
        let row_of = |v: &Vec3| (((v.y - oy) / cell_mm).floor() as i64).clamp(0, h as i64 - 1);
        let mut row_starts = vec![0usize; h + 1];
        for v in &world {
            row_starts[row_of(v) as usize + 1] += 1;
        }
        for r in 0..h {
            row_starts[r + 1] += row_starts[r];
        }
        let mut fill = row_starts.clone();
        let mut by_row = vec![0u32; world.len()];
        for (i, v) in world.iter().enumerate() {
            let r = row_of(v) as usize;
            by_row[fill[r]] = i as u32;
            fill[r] += 1;
        }

        // Push each vertex's ball into the cells it can reach, keeping the
        // lowest Z. Only the minimum is kept, so the update is a compare and a
        // store - no lists, no merging, no sorting. A cell's minimum does not
        // depend on the order its candidates arrive in, so bands of rows fill
        // in parallel, each from the vertices within reach of its rows, and
        // give the sequential result.
        const BAND_ROWS: usize = 32;
        let mut lo = vec![NO_SOLID; w * h];
        lo.par_chunks_mut(BAND_ROWS * w)
            .enumerate()
            .for_each(|(band, cells)| {
                let r0 = (band * BAND_ROWS) as i64;
                let r1 = r0 + (cells.len() / w) as i64; // exclusive
                let first = (r0 - rc).max(0) as usize;
                let last = ((r1 - 1 + rc).min(h as i64 - 1)) as usize;
                for &i in &by_row[row_starts[first]..row_starts[last + 1]] {
                    let v = world[i as usize];
                    let (vx, vy, vz) = (v.x, v.y, v.z);
                    let ix = ((vx - ox) / cell_mm).floor() as i64;
                    let iy = ((vy - oy) / cell_mm).floor() as i64;
                    let ny_end = js_min((h - 1) as f64, (iy + rc) as f64) as i64;
                    let nx_end = js_min((w - 1) as f64, (ix + rc) as f64) as i64;
                    for ny in (0.max(iy - rc)).max(r0)..=ny_end.min(r1 - 1) {
                        let dy = oy + ny as f64 * cell_mm - vy;
                        for nx in (0.max(ix - rc))..=nx_end {
                            let dx = ox + nx as f64 * cell_mm - vx;
                            let lat2 = dx * dx + dy * dy;
                            if lat2 >= r2 {
                                continue;
                            }
                            let candidate = vz - (r2 - lat2).sqrt();
                            let at = (ny - r0) as usize * w + nx as usize;
                            // The store is a Float32Array; the compare reads the stored value.
                            if candidate < f64::from(cells[at]) {
                                cells[at] = candidate as f32;
                            }
                        }
                    }
                }
            });

        Some(ColumnClearanceMap {
            stats: ColumnClearanceMapStats {
                cell_mm,
                width: w,
                height: h,
                cells: w * h,
                radius_mm: radius,
                vertex_count: positions.len(),
                bytes: lo.len() * 4,
            },
            cell: cell_mm,
            ox,
            oy,
            w,
            h,
            lo,
        })
    }

    fn cell_of(&self, x: f64, y: f64) -> Option<usize> {
        let ix = ((x - self.ox) / self.cell).floor();
        let iy = ((y - self.oy) / self.cell).floor();
        if ix < 0.0
            || ix >= self.w as f64
            || iy < 0.0
            || iy >= self.h as f64
            || ix.is_nan()
            || iy.is_nan()
        {
            return None;
        }
        Some(iy as usize * self.w + ix as usize)
    }

    /// The lowest Z known to be blocked at this XY, or `None` off the grid. A
    /// point at `(x, y, lowestBlockedZ(x, y))` is within `clearance` of the
    /// model, so it is a real blocked Z.
    pub fn lowest_blocked_z(&self, x: f64, y: f64) -> Option<f64> {
        let value = self.lo[self.cell_of(x, y)?];
        if value == NO_SOLID {
            None
        } else {
            Some(f64::from(value))
        }
    }

    /// `Blocked` when a real blocked Z lies inside the column, else `Unknown`.
    /// Callers may treat `Blocked` as the march's answer.
    pub fn column_verdict(&self, x: f64, y: f64, z_top: f64, z_bot: f64) -> ColumnVerdict {
        let Some(c) = self.cell_of(x, y) else {
            return ColumnVerdict::Unknown;
        };
        let lo = self.lo[c];
        if lo == NO_SOLID {
            return ColumnVerdict::Unknown;
        }
        let lo = f64::from(lo);
        let bottom = if z_top < z_bot { z_top } else { z_bot };
        let top = if z_top < z_bot { z_bot } else { z_top };
        // `lo` is a point of the blocked set, so it settles the column only if
        // it lies inside it. Below the bottom it is outside the column; above
        // the top it is a different Z and says nothing.
        if lo >= bottom && lo <= top {
            ColumnVerdict::Blocked
        } else {
            ColumnVerdict::Unknown
        }
    }
}
