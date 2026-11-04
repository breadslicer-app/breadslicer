/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where an island finding points, and where its support goes: the pixel of
//! the island's *unheld* part (its pixels with nothing cured straight below)
//! farthest from both the island's edge and the pixels of it that already
//! rest on something.
//!
//! That is the middle of the widest stretch nothing holds yet: a support tip
//! there takes the most load off the island, and it never lands on a post or
//! a tip that is already there. For an island with nothing below at all it
//! is the middle of the island, on the island even when it is a ring.
//!
//! Distances are chamfer distances (3 for a step along a row or a column, 4
//! for a diagonal step), close to Euclidean and exact in integers. Ties go
//! to the pixel nearest the centroid of the candidates, then to the first in
//! scan order, so the choice is deterministic.

use crate::runs::Run;

use super::pieces::{intersect, subtract};

/// The largest bounding box, in pixels, that gets a distance map; beyond it
/// the candidate nearest the centroid is taken instead. 2^24 cells (32 MiB),
/// a 4096 x 4096 pixel island, 78 mm square on a 19 µm screen.
const MAX_GRID_CELLS: usize = 1 << 24;

/// The deepest unheld pixel of `piece` (the runs of one piece), where `below`
/// is the solid of the layer below. Pixels of `prefer` (the model, for a
/// model island, where a support tip can go) are taken when the unheld part
/// has any. Returns the pixel and whether it is in `prefer`, or `None` when
/// every pixel of `piece` rests on `below`.
pub(super) fn deepest_unheld(
    piece: &[Run],
    below: &[Run],
    prefer: Option<&[Run]>,
) -> Option<([u32; 2], bool)> {
    let _clock = crate::clock::scope("islands.focus");
    let held = intersect(piece, below);
    let unheld = subtract(piece, &held);
    if unheld.is_empty() {
        return None;
    }
    let preferred = prefer.map_or_else(Vec::new, |prefer| intersect(&unheld, prefer));
    let (candidates, is_preferred) = if preferred.is_empty() {
        (unheld, false)
    } else {
        (preferred, true)
    };
    let centroid = Centroid::of(&candidates);
    let pixel = match DistanceMap::new(piece, &held) {
        Some(map) => map.deepest(&candidates, &centroid),
        None => centroid.nearest(&candidates),
    };
    Some((pixel, is_preferred))
}

/// Twice the coordinate sums of the candidates' pixel centres, and their
/// number: the centroid, kept exact in integers.
struct Centroid {
    twice_x: i128,
    twice_y: i128,
    count: i128,
}

impl Centroid {
    fn of(runs: &[Run]) -> Self {
        let mut centroid = Self {
            twice_x: 0,
            twice_y: 0,
            count: 0,
        };
        for run in runs {
            let len = i128::from(run.len());
            centroid.twice_x += len * i128::from(run.x0 + run.x1);
            centroid.twice_y += len * (2 * i128::from(run.row) + 1);
            centroid.count += len;
        }
        centroid
    }

    /// The squared distance from pixel `(x, y)`'s centre to the centroid,
    /// scaled by `(2 * count)²`.
    fn distance(&self, x: u32, y: u32) -> i128 {
        let dx = (2 * i128::from(x) + 1) * self.count - self.twice_x;
        let dy = (2 * i128::from(y) + 1) * self.count - self.twice_y;
        dx * dx + dy * dy
    }

    /// The pixel of `runs` nearest the centroid, the first in scan order on a
    /// tie.
    fn nearest(&self, runs: &[Run]) -> [u32; 2] {
        // Along a row the distance is smallest at the column whose pixel
        // holds the centroid's x, or the one before it on a tie.
        let column =
            u32::try_from((self.twice_x / (2 * self.count).max(1)).max(0)).unwrap_or(u32::MAX);
        let mut best: Option<(i128, [u32; 2])> = None;
        for run in runs {
            let x = column.clamp(run.x0, run.x1 - 1);
            for x in [x.saturating_sub(1).max(run.x0), x] {
                let d = self.distance(x, run.row);
                if best.is_none_or(|(near, _)| d < near) {
                    best = Some((d, [x, run.row]));
                }
            }
        }
        best.map_or([0, 0], |(_, pixel)| pixel)
    }
}

/// Chamfer distances from every pixel of a piece's bounding box (plus a
/// one-pixel border) to the nearest pixel outside the piece or held.
struct DistanceMap {
    x0: u32,
    y0: u32,
    width: usize,
    cells: Vec<u16>,
}

impl DistanceMap {
    /// The map of `piece` with `held` (part of it) as zero, or `None` when
    /// the piece is too large for one.
    fn new(piece: &[Run], held: &[Run]) -> Option<Self> {
        let x0 = piece.iter().map(|r| r.x0).min()?;
        let x1 = piece.iter().map(|r| r.x1).max()?;
        let y0 = piece.first()?.row;
        let y1 = piece.last()?.row + 1;
        let width = (x1 - x0) as usize + 2;
        let height = (y1 - y0) as usize + 2;
        if width.checked_mul(height)? > MAX_GRID_CELLS {
            return None;
        }
        let mut map = Self {
            x0,
            y0,
            width,
            cells: vec![0; width * height],
        };
        for run in piece {
            map.fill(run, u16::MAX);
        }
        for run in held {
            map.fill(run, 0);
        }
        map.sweep(height);
        Some(map)
    }

    /// The cell of pixel `(x, y)`, which lies in the bounding box.
    fn cell(&self, x: u32, y: u32) -> usize {
        (y - self.y0 + 1) as usize * self.width + (x - self.x0 + 1) as usize
    }

    fn fill(&mut self, run: &Run, value: u16) {
        let start = self.cell(run.x0, run.row);
        self.cells[start..start + run.len() as usize].fill(value);
    }

    /// The two chamfer passes. Border cells stay 0.
    fn sweep(&mut self, height: usize) {
        let w = self.width;
        for y in 1..height - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                if self.cells[i] == 0 {
                    continue;
                }
                let best = [
                    self.cells[i - 1].saturating_add(3),
                    self.cells[i - w].saturating_add(3),
                    self.cells[i - w - 1].saturating_add(4),
                    self.cells[i - w + 1].saturating_add(4),
                ];
                self.cells[i] = best.into_iter().fold(self.cells[i], u16::min);
            }
        }
        for y in (1..height - 1).rev() {
            for x in (1..w - 1).rev() {
                let i = y * w + x;
                if self.cells[i] == 0 {
                    continue;
                }
                let best = [
                    self.cells[i + 1].saturating_add(3),
                    self.cells[i + w].saturating_add(3),
                    self.cells[i + w + 1].saturating_add(4),
                    self.cells[i + w - 1].saturating_add(4),
                ];
                self.cells[i] = best.into_iter().fold(self.cells[i], u16::min);
            }
        }
    }

    /// The candidate farthest from the edge and the held pixels; nearest the
    /// centroid on a tie, then first in scan order.
    fn deepest(&self, candidates: &[Run], centroid: &Centroid) -> [u32; 2] {
        let mut best: Option<(u16, i128, [u32; 2])> = None;
        for run in candidates {
            for x in run.x0..run.x1 {
                let depth = self.cells[self.cell(x, run.row)];
                let better = match best {
                    None => true,
                    Some((d, _, _)) if depth != d => depth > d,
                    Some((_, near, _)) => centroid.distance(x, run.row) < near,
                };
                if better {
                    best = Some((depth, centroid.distance(x, run.row), [x, run.row]));
                }
            }
        }
        best.map_or([candidates[0].x0, candidates[0].row], |(_, _, pixel)| pixel)
    }
}
