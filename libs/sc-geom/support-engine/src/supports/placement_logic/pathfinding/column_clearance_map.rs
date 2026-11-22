//! For each cell of a grid over the model seen from above, the lowest
//! height a ball of the clearance radius round any of the model's vertices
//! reaches. It answers "is this column down to the plate blocked?" without
//! marching the distance field.
//!
//! Vertices, not triangles: a vertex is on the solid, so a ball round it is
//! within the clearance of the model, and a cell's lowest such point is a
//! blocked height for sure. A column below the model is blocked exactly when
//! a blocked height lies in it, and with the lowest one known that is a
//! single comparison. The map cannot prove a column clear (a point can sit
//! near a triangle's middle, far from its corners): it says blocked or does
//! not know.

use rayon::prelude::*;

use crate::geom::math_utils::{js_max, js_min};
use crate::geom::{Matrix4, Vec3};

/// What a column check can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnVerdict {
    Blocked,
    Unknown,
}

/// The size of a built map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnClearanceMapStats {
    pub cell_mm: f64,
    pub width: usize,
    pub height: usize,
    pub cells: usize,
    /// The radius the balls were drawn with: the clearance less a query's
    /// snap to its cell's corner.
    pub radius_mm: f64,
    pub vertex_count: usize,
    pub bytes: usize,
}

/// A map past this many cells is not built.
const MOST_CELLS: f64 = 12_000_000.0;
/// Taken off the radius past the snap, mm.
const SLACK_MM: f64 = 0.02;
/// A cell no ball reaches.
const EMPTY: f32 = f32::INFINITY;
/// Rows filled by one parallel job.
const ROWS_PER_JOB: usize = 32;

/// See the module notes.
#[derive(Debug, Clone)]
pub struct ColumnClearanceMap {
    pub stats: ColumnClearanceMapStats,
    cell: f64,
    origin: (f64, f64),
    width: usize,
    height: usize,
    lowest: Vec<f32>,
}

impl ColumnClearanceMap {
    /// The map of the vertices `positions` under `matrix_world`, with cells of
    /// `cell_mm`; `None` when it would be too big. A query snaps to its
    /// cell's corner, so the balls shrink by half a cell's diagonal to stay
    /// within the clearance of the point asked about.
    pub fn build(
        positions: &[[f32; 3]],
        matrix_world: &Matrix4,
        clearance_mm: f64,
        cell_mm: f64,
    ) -> Option<Self> {
        let points: Vec<Vec3> = positions
            .par_iter()
            .map(|p| {
                Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
                    .apply_matrix4(matrix_world)
            })
            .collect();
        let mut lo = (f64::INFINITY, f64::INFINITY);
        let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &points {
            if p.x < lo.0 {
                lo.0 = p.x;
            }
            if p.y < lo.1 {
                lo.1 = p.y;
            }
            if p.x > hi.0 {
                hi.0 = p.x;
            }
            if p.y > hi.1 {
                hi.1 = p.y;
            }
        }
        let w = ((hi.0 - lo.0) / cell_mm).ceil() + 2.0;
        let h = ((hi.1 - lo.1) / cell_mm).ceil() + 2.0;
        if !(w * h).is_finite() || w * h > MOST_CELLS {
            return None;
        }
        let (width, height) = (w as usize, h as usize);
        let origin = (lo.0 - cell_mm, lo.1 - cell_mm);
        let radius = js_max(
            0.01,
            clearance_mm - (cell_mm * std::f64::consts::FRAC_1_SQRT_2 + SLACK_MM),
        );
        let r2 = radius * radius;
        let reach = (radius / cell_mm).ceil() as i64;

        // The vertices sorted by row (counting sort): a job finds those that
        // reach its rows at once.
        let row = |p: &Vec3| {
            (((p.y - origin.1) / cell_mm).floor() as i64).clamp(0, height as i64 - 1) as usize
        };
        let mut starts = vec![0usize; height + 1];
        for p in &points {
            starts[row(p) + 1] += 1;
        }
        for r in 0..height {
            starts[r + 1] += starts[r];
        }
        let mut next = starts.clone();
        let mut sorted = vec![0u32; points.len()];
        for (i, p) in points.iter().enumerate() {
            let r = row(p);
            sorted[next[r]] = i as u32;
            next[r] += 1;
        }

        // Each ball lowers the cells it covers; only a minimum is kept, so
        // the order balls arrive in does not matter and bands of rows fill
        // in parallel.
        let mut lowest = vec![EMPTY; width * height];
        lowest
            .par_chunks_mut(ROWS_PER_JOB * width)
            .enumerate()
            .for_each(|(job, cells)| {
                let first_row = (job * ROWS_PER_JOB) as i64;
                let end_row = first_row + (cells.len() / width) as i64;
                let from = (first_row - reach).max(0) as usize;
                let to = ((end_row - 1 + reach).min(height as i64 - 1)) as usize;
                for &i in &sorted[starts[from]..starts[to + 1]] {
                    let p = points[i as usize];
                    let cx = ((p.x - origin.0) / cell_mm).floor() as i64;
                    let cy = ((p.y - origin.1) / cell_mm).floor() as i64;
                    let last_y = js_min((height - 1) as f64, (cy + reach) as f64) as i64;
                    let last_x = js_min((width - 1) as f64, (cx + reach) as f64) as i64;
                    for y in (cy - reach).max(0).max(first_row)..=last_y.min(end_row - 1) {
                        let dy = origin.1 + y as f64 * cell_mm - p.y;
                        for x in (cx - reach).max(0)..=last_x {
                            let dx = origin.0 + x as f64 * cell_mm - p.x;
                            let across2 = dx * dx + dy * dy;
                            if across2 >= r2 {
                                continue;
                            }
                            let low = p.z - (r2 - across2).sqrt();
                            let at = (y - first_row) as usize * width + x as usize;
                            if low < f64::from(cells[at]) {
                                cells[at] = low as f32;
                            }
                        }
                    }
                }
            });

        Some(ColumnClearanceMap {
            stats: ColumnClearanceMapStats {
                cell_mm,
                width,
                height,
                cells: width * height,
                radius_mm: radius,
                vertex_count: positions.len(),
                bytes: lowest.len() * 4,
            },
            cell: cell_mm,
            origin,
            width,
            height,
            lowest,
        })
    }

    /// The cell holding (`x`, `y`), if inside the map.
    fn cell_at(&self, x: f64, y: f64) -> Option<usize> {
        let cx = ((x - self.origin.0) / self.cell).floor();
        let cy = ((y - self.origin.1) / self.cell).floor();
        let inside = cx >= 0.0 && cx < self.width as f64 && cy >= 0.0 && cy < self.height as f64;
        inside.then(|| cy as usize * self.width + cx as usize)
    }

    /// [`ColumnVerdict::Blocked`] when a blocked height lies in the column at
    /// (`x`, `y`) between `z_top` and `z_bot` (either order), else
    /// [`ColumnVerdict::Unknown`].
    pub fn column_verdict(&self, x: f64, y: f64, z_top: f64, z_bot: f64) -> ColumnVerdict {
        let Some(at) = self.cell_at(x, y) else {
            return ColumnVerdict::Unknown;
        };
        let low = self.lowest[at];
        if low == EMPTY {
            return ColumnVerdict::Unknown;
        }
        let low = f64::from(low);
        let (bottom, top) = if z_top < z_bot {
            (z_top, z_bot)
        } else {
            (z_bot, z_top)
        };
        if low >= bottom && low <= top {
            ColumnVerdict::Blocked
        } else {
            ColumnVerdict::Unknown
        }
    }
}
