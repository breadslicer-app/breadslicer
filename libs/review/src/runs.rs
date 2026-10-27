/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Row runs: the compact pixel form every layer is analysed in.
//!
//! A full bitmap of a 16K printer is 59 million pixels per layer, so the
//! review never allocates one. A layer is a list of [`Run`]s instead: one
//! entry per horizontal stretch of lit pixels on a raster row.
//!
//! Every function here takes and returns a *run list*: runs sorted by
//! `(row, x0)`, each non-empty, and within a row disjoint and not touching
//! (`x1 < next.x0`). Lists built by this module always hold that invariant.

use sc_sl1cer::raster::rle::LayerRaster;

/// A horizontal stretch of pixels on one raster row: pixels `x0..x1` of row
/// `row` (half-open, so `x1 - x0` pixels). Pixel `(x, y)` covers the square
/// `[x, x + 1) x [y, y + 1)` of raster space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Run {
    /// The raster row, `0..height_px`.
    pub row: u32,
    /// The first pixel of the run.
    pub x0: u32,
    /// One past the last pixel of the run.
    pub x1: u32,
}

impl Run {
    /// Builds the run `x0..x1` of `row`.
    #[inline]
    pub const fn new(row: u32, x0: u32, x1: u32) -> Self {
        Self { row, x0, x1 }
    }

    /// The number of pixels in the run.
    #[inline]
    pub const fn len(&self) -> u32 {
        self.x1 - self.x0
    }

    /// Whether the run holds no pixel. Run lists never contain such a run.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.x1 <= self.x0
    }
}

/// The number of pixels in a run list.
pub fn area_px(runs: &[Run]) -> u64 {
    runs.iter().map(|run| u64::from(run.len())).sum()
}

/// Turns a layer raster into the run list of its lit pixels (any value other
/// than 0). Raster runs may wrap from the end of one row into the next; they
/// are split at row ends. Neighbouring lit raster runs of different values
/// (an anti-aliased edge) merge into one run. Pixels past the raster's last
/// row are dropped.
pub fn runs_from_raster(raster: &LayerRaster) -> Vec<Run> {
    let width = u64::from(raster.width_px());
    let height = u64::from(raster.height_px());
    let mut runs = Vec::new();
    if width == 0 {
        return runs;
    }
    let limit = width * height;
    let mut pos = 0u64;
    for rle in raster.runs() {
        let start = pos;
        pos += u64::from(rle.length);
        if rle.value == 0 {
            continue;
        }
        let end = pos.min(limit);
        let mut at = start;
        while at < end {
            let row = at / width;
            let row_start = row * width;
            let seg_end = end.min(row_start + width);
            // Every value fits in u32: rows < height and columns < width,
            // both u32 in the raster.
            let run = Run::new(
                row as u32,
                (at - row_start) as u32,
                (seg_end - row_start) as u32,
            );
            match runs.last_mut() {
                Some(last) if last.row == run.row && last.x1 == run.x0 => last.x1 = run.x1,
                _ => runs.push(run),
            }
            at = seg_end;
        }
    }
    runs
}

/// The union of two run lists. Overlapping or touching runs on a row merge
/// into one.
pub fn union(a: &[Run], b: &[Run]) -> Vec<Run> {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a.to_vec();
    }
    let mut out: Vec<Run> = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next = match (a.get(i), b.get(j)) {
            (Some(ra), Some(rb)) if (ra.row, ra.x0) <= (rb.row, rb.x0) => {
                i += 1;
                *ra
            }
            (Some(_), Some(rb)) => {
                j += 1;
                *rb
            }
            (Some(ra), None) => {
                i += 1;
                *ra
            }
            (None, Some(rb)) => {
                j += 1;
                *rb
            }
            (None, None) => break,
        };
        match out.last_mut() {
            Some(last) if last.row == next.row && next.x0 <= last.x1 => {
                last.x1 = last.x1.max(next.x1);
            }
            _ => out.push(next),
        }
    }
    out
}

/// The pixels of the `width_px` x `height_px` raster that `solid` leaves
/// unlit, as a run list. A row without any solid run is one full-width run.
pub fn complement(solid: &[Run], width_px: u32, height_px: u32) -> Vec<Run> {
    let mut out = Vec::with_capacity(solid.len() + height_px as usize);
    let mut i = 0;
    for row in 0..height_px {
        let mut x = 0;
        while let Some(run) = solid.get(i) {
            if run.row != row {
                break;
            }
            if run.x0 > x {
                out.push(Run::new(row, x, run.x0));
            }
            x = run.x1;
            i += 1;
        }
        if x < width_px {
            out.push(Run::new(row, x, width_px));
        }
    }
    out
}

/// Calls `on_overlap(index_in_a, index_in_b, pixels)` for every pair of runs,
/// one from each list, that share pixels, in `(row, x)` order.
pub fn for_each_overlap(a: &[Run], b: &[Run], mut on_overlap: impl FnMut(usize, usize, u32)) {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ra, rb) = (a[i], b[j]);
        if ra.row != rb.row {
            if ra.row < rb.row {
                i += 1;
            } else {
                j += 1;
            }
            continue;
        }
        let lo = ra.x0.max(rb.x0);
        let hi = ra.x1.min(rb.x1);
        if lo < hi {
            on_overlap(i, j, hi - lo);
        }
        // Drop whichever run ends first; the other may still meet the next.
        if ra.x1 <= rb.x1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The number of pixels two run lists share.
pub fn overlap_px(a: &[Run], b: &[Run]) -> u64 {
    let mut total = 0u64;
    for_each_overlap(a, b, |_, _, px| total += u64::from(px));
    total
}

#[cfg(test)]
mod runs_tests {
    use sc_sl1cer::raster::rle::{LayerRaster, RleRun};

    use super::*;

    fn rle(value: u8, length: u32) -> RleRun {
        RleRun { value, length }
    }

    #[test]
    fn raster_runs_split_at_row_ends() {
        // 4 x 3 raster: row 0 = ..##, row 1 = #..., row 2 = ....
        // The lit run wraps from the end of row 0 into row 1.
        let raster = LayerRaster::new(0.0, 4, 3, vec![rle(0, 2), rle(255, 3), rle(0, 7)]);
        assert_eq!(
            runs_from_raster(&raster),
            vec![Run::new(0, 2, 4), Run::new(1, 0, 1)]
        );
    }

    #[test]
    fn raster_runs_ignore_pixels_past_the_last_row() {
        let raster = LayerRaster::new(0.0, 2, 1, vec![rle(255, 5)]);
        assert_eq!(runs_from_raster(&raster), vec![Run::new(0, 0, 2)]);
    }

    #[test]
    fn union_merges_overlapping_and_touching_runs() {
        let a = [Run::new(0, 0, 3), Run::new(1, 5, 6)];
        let b = [Run::new(0, 3, 4), Run::new(0, 6, 8), Run::new(1, 0, 2)];
        assert_eq!(
            union(&a, &b),
            vec![
                Run::new(0, 0, 4),
                Run::new(0, 6, 8),
                Run::new(1, 0, 2),
                Run::new(1, 5, 6),
            ]
        );
    }

    #[test]
    fn complement_fills_empty_rows_and_gaps() {
        let solid = [Run::new(1, 0, 2), Run::new(1, 3, 5)];
        assert_eq!(
            complement(&solid, 5, 3),
            vec![Run::new(0, 0, 5), Run::new(1, 2, 3), Run::new(2, 0, 5)]
        );
    }

    #[test]
    fn overlaps_pair_every_shared_stretch() {
        let a = [Run::new(0, 0, 10)];
        let b = [Run::new(0, 2, 4), Run::new(0, 6, 12), Run::new(1, 0, 10)];
        let mut seen = Vec::new();
        for_each_overlap(&a, &b, |i, j, px| seen.push((i, j, px)));
        assert_eq!(seen, vec![(0, 0, 2), (0, 1, 4)]);
        assert_eq!(overlap_px(&a, &b), 6);
    }
}
