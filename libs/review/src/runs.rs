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

/// Every pixel within `r` of `runs` along both axes (a square), clipped to a
/// grid `width` by `height`.
pub fn dilate_square(runs: &[Run], r: u32, width: u32, height: u32) -> Vec<Run> {
    let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
        return Vec::new();
    };
    let rows = RowIndex::new(runs, height);
    let mut out = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for y in first.row.saturating_sub(r)..=last.row.saturating_add(r).min(height - 1) {
        spans.clear();
        for source in y.saturating_sub(r)..=y.saturating_add(r).min(height - 1) {
            spans.extend(
                rows.row(runs, source)
                    .iter()
                    .map(|run| (run.x0.saturating_sub(r), (run.x1 + r).min(width))),
            );
        }
        spans.sort_unstable();
        let mut spans = spans.iter();
        let Some(&(mut x0, mut x1)) = spans.next() else {
            continue;
        };
        for &(a, b) in spans {
            if a <= x1 {
                x1 = x1.max(b);
            } else {
                out.push(Run::new(y, x0, x1));
                (x0, x1) = (a, b);
            }
        }
        out.push(Run::new(y, x0, x1));
    }
    out
}

/// The pixels of `runs`, sorted by row, around which an ellipse of radii `rx`
/// and `ry` (in pixels, along each axis) lies entirely on `runs`: a pixel
/// counts as covered when its centre is inside the ellipse.
pub fn erode_ellipse(runs: &[Run], rx: f64, ry: f64) -> Vec<Run> {
    let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
        return Vec::new();
    };
    let reach_y = ry.floor() as u32;
    // Each row's runs, from the first row on.
    let mut rows = vec![&runs[..0]; (last.row - first.row + 1) as usize];
    for chunk in runs.chunk_by(|a, b| a.row == b.row) {
        rows[(chunk[0].row - first.row) as usize] = chunk;
    }
    // How far an ellipse reaches along a row `dy` rows from its centre.
    let reach_x: Vec<u32> = (0..=reach_y)
        .map(|dy| {
            let along = if ry > 0.0 { f64::from(dy) / ry } else { 0.0 };
            (rx * (1.0 - along * along).max(0.0).sqrt()).floor() as u32
        })
        .collect();
    let row_at = |y: i64| -> &[Run] {
        usize::try_from(y - i64::from(first.row))
            .ok()
            .and_then(|at| rows.get(at).copied())
            .unwrap_or(&[])
    };
    let mut out = Vec::new();
    let (mut kept, mut next): (Vec<(u32, u32)>, Vec<(u32, u32)>) = (Vec::new(), Vec::new());
    for y in first.row..=last.row {
        kept.clear();
        kept.extend(
            row_at(i64::from(y))
                .iter()
                .map(|run| (run.x0 + reach_x[0], run.x1.saturating_sub(reach_x[0])))
                .filter(|(a, b)| a < b),
        );
        for dy in 1..=reach_y {
            let reach = reach_x[dy as usize];
            for side in [i64::from(y) - i64::from(dy), i64::from(y) + i64::from(dy)] {
                if kept.is_empty() {
                    break;
                }
                next.clear();
                for run in row_at(side) {
                    let (a, b) = (run.x0 + reach, run.x1.saturating_sub(reach));
                    next.extend(
                        kept.iter()
                            .map(|&(k0, k1)| (k0.max(a), k1.min(b)))
                            .filter(|(from, to)| from < to),
                    );
                }
                next.sort_unstable();
                std::mem::swap(&mut kept, &mut next);
            }
        }
        out.extend(kept.iter().map(|&(x0, x1)| Run::new(y, x0, x1)));
    }
    out
}

/// The pixels in both run lists.
pub fn intersection(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::new();
    for_each_overlap(a, b, |i, j, _| {
        out.push(Run::new(
            a[i].row,
            a[i].x0.max(b[j].x0),
            a[i].x1.min(b[j].x1),
        ));
    });
    out
}

/// The pixels of run list `a` that are not in run list `b`.
pub fn difference(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::with_capacity(a.len());
    let mut j = 0;
    for run in a {
        while j < b.len() && (b[j].row, b[j].x1) <= (run.row, run.x0) {
            j += 1;
        }
        let mut x = run.x0;
        for cut in b[j..]
            .iter()
            .take_while(|c| c.row == run.row && c.x0 < run.x1)
        {
            if cut.x0 > x {
                out.push(Run::new(run.row, x, cut.x0));
            }
            x = x.max(cut.x1);
        }
        if x < run.x1 {
            out.push(Run::new(run.row, x, run.x1));
        }
    }
    out
}

/// The morphological opening of the run list `runs` by a `width` x `width`
/// square, on a screen `height_px` rows tall: the pixels of `runs` that a
/// square of that size lying entirely in `runs` covers. Stretches narrower
/// than the square in either direction disappear; wider ones keep their
/// shape. A `width` of 0 or 1 changes nothing.
pub fn open(runs: &[Run], width: u32, height_px: u32) -> Vec<Run> {
    if width <= 1 || runs.is_empty() {
        return runs.to_vec();
    }
    let reach = width - 1;
    // Erode: the top-left corners of the squares that fit. A run keeps the
    // corners of the squares that fit in it on its row; `width` rows in a
    // row must all keep a corner.
    let corners: Vec<Run> = runs
        .iter()
        .filter(|run| run.len() >= width)
        .map(|run| Run::new(run.row, run.x0, run.x1 - reach))
        .collect();
    let rows = RowIndex::new(&corners, height_px);
    let mut eroded: Vec<Run> = Vec::with_capacity(corners.len());
    let (mut kept, mut scratch): (Vec<Run>, Vec<Run>) = (Vec::new(), Vec::new());
    for row in 0..height_px.saturating_sub(reach) {
        let first = rows.row(&corners, row);
        if first.is_empty() {
            continue;
        }
        kept.clear();
        kept.extend_from_slice(first);
        for next in row + 1..=row + reach {
            intersect_row(&kept, rows.row(&corners, next), &mut scratch);
            std::mem::swap(&mut kept, &mut scratch);
            if kept.is_empty() {
                break;
            }
        }
        eroded.extend_from_slice(&kept);
    }
    // Dilate: every square back from its corner.
    for run in &mut eroded {
        run.x1 += reach;
    }
    let spans = eroded;
    let rows = RowIndex::new(&spans, height_px);
    let mut opened: Vec<Run> = Vec::with_capacity(runs.len());
    let (mut covered, mut scratch): (Vec<Run>, Vec<Run>) = (Vec::new(), Vec::new());
    for row in 0..height_px {
        covered.clear();
        for from in row.saturating_sub(reach)..=row {
            union_row(&covered, rows.row(&spans, from), &mut scratch);
            std::mem::swap(&mut covered, &mut scratch);
        }
        opened.extend(covered.iter().map(|span| Run::new(row, span.x0, span.x1)));
    }
    opened
}

/// The runs of `a` (on one row) clipped to the runs of `b` (on another),
/// kept on `a`'s row, into `out`.
fn intersect_row(a: &[Run], b: &[Run], out: &mut Vec<Run>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (lo, hi) = (a[i].x0.max(b[j].x0), a[i].x1.min(b[j].x1));
        if lo < hi {
            out.push(Run::new(a[i].row, lo, hi));
        }
        if a[i].x1 <= b[j].x1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The union of the runs of `a` and `b` (each sorted, on any rows), as
/// sorted runs that neither overlap nor touch, into `out`. Their rows are
/// left as found: the caller sets the row.
fn union_row(a: &[Run], b: &[Run], out: &mut Vec<Run>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next = if j >= b.len() || (i < a.len() && a[i].x0 <= b[j].x0) {
            i += 1;
            a[i - 1]
        } else {
            j += 1;
            b[j - 1]
        };
        match out.last_mut() {
            Some(last) if next.x0 <= last.x1 => last.x1 = last.x1.max(next.x1),
            _ => out.push(next),
        }
    }
}

/// Where each row's runs start in a run list.
struct RowIndex {
    /// `starts[row]..starts[row + 1]` are the runs of `row`.
    starts: Vec<usize>,
}

impl RowIndex {
    fn new(runs: &[Run], height_px: u32) -> Self {
        let mut starts = Vec::with_capacity(height_px as usize + 1);
        let mut at = 0;
        for row in 0..=height_px {
            while at < runs.len() && runs[at].row < row {
                at += 1;
            }
            starts.push(at);
        }
        Self { starts }
    }

    fn row<'r>(&self, runs: &'r [Run], row: u32) -> &'r [Run] {
        let row = row as usize;
        &runs[self.starts[row]..self.starts[row + 1]]
    }
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

    /// The run list of a picture: `#` is in the list.
    fn picture(rows: &[&str]) -> Vec<Run> {
        let mut runs = Vec::new();
        for (row, line) in rows.iter().enumerate() {
            let mut start = None;
            for (x, c) in line.chars().chain(std::iter::once('.')).enumerate() {
                match (c == '#', start) {
                    (true, None) => start = Some(x as u32),
                    (false, Some(x0)) => {
                        runs.push(Run::new(row as u32, x0, x as u32));
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        runs
    }

    #[test]
    fn opening_drops_what_is_narrower_than_the_square() {
        let runs = picture(&[
            "####.......",
            "####.......",
            "#########..",
            "####.......",
            "##.....####",
            "##.....####",
            "##.....####",
        ]);
        // The 1-pixel spur and the 2-pixel column go; the blocks stay whole.
        assert_eq!(
            open(&runs, 3, 7),
            picture(&[
                "####.......",
                "####.......",
                "####.......",
                "####.......",
                ".......####",
                ".......####",
                ".......####",
            ])
        );
        assert_eq!(open(&runs, 1, 7), runs);
        assert_eq!(open(&runs, 5, 7), Vec::new());
    }

    #[test]
    fn opening_keeps_every_pixel_a_square_covers() {
        // A staircase of overlapping 3 x 3 squares stays whole; the pixel
        // that only touches it by a corner, and the 2 x 2 step, do not.
        let runs = picture(&[
            "###.......",
            "###.......",
            "######....",
            "...###....",
            "...######.",
            "......###.",
            "......###.",
            ".........#",
            "##.......#",
            "##........",
        ]);
        assert_eq!(
            open(&runs, 3, 10),
            picture(&[
                "###.......",
                "###.......",
                "######....",
                "...###....",
                "...######.",
                "......###.",
                "......###.",
                "..........",
                "..........",
                "..........",
            ])
        );
    }
}
