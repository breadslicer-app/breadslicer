/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A grid of square cells, one bit each, stored row by row in 64-bit words.
//!
//! Every fill is conservative: a cell is set when the shape touches it, so
//! what the grid holds always covers the shape.

/// A grid of `width` × `height` cells, one bit each.
#[derive(Clone, Debug)]
pub(crate) struct BitGrid {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Words per row.
    pub(crate) words: usize,
    bits: Vec<u64>,
}

impl BitGrid {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let words = width.div_ceil(64).max(1);
        Self {
            width,
            height,
            words,
            bits: vec![0; words * height],
        }
    }

    pub(crate) fn row(&self, y: usize) -> &[u64] {
        &self.bits[y * self.words..(y + 1) * self.words]
    }

    pub(crate) fn row_mut(&mut self, y: usize) -> &mut [u64] {
        &mut self.bits[y * self.words..(y + 1) * self.words]
    }

    #[cfg(test)]
    pub(crate) fn get(&self, x: usize, y: usize) -> bool {
        self.row(y)[x / 64] >> (x % 64) & 1 == 1
    }

    /// Sets the cells `x0..=x1` of row `y`, clipped to the grid.
    pub(crate) fn set_span(&mut self, y: usize, x0: usize, x1: usize) {
        let x1 = x1.min(self.width.saturating_sub(1));
        if x0 > x1 || y >= self.height {
            return;
        }
        set_bits(self.row_mut(y), x0, x1 + 1);
    }

    /// Sets every cell the convex polygon `poly` touches, its corners given
    /// in cells (`[1.5, 0.0]` is the middle of the second cell's bottom edge).
    /// A polygon seen edge-on, a segment or a point, sets the cells it
    /// touches too: a wall seen from above is a line.
    pub(crate) fn fill_convex(&mut self, poly: &[[f64; 2]]) {
        if poly.is_empty() || self.height == 0 {
            return;
        }
        let (mut y_lo, mut y_hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in poly {
            y_lo = y_lo.min(p[1]);
            y_hi = y_hi.max(p[1]);
        }
        if !(y_lo.is_finite() && y_hi.is_finite()) {
            return;
        }
        let max_row = (self.height - 1) as f64;
        if y_hi < 0.0 || y_lo > max_row + 1.0 {
            return;
        }
        // A cell that the shape only touches along its edge is left out:
        // that edge belongs to the cell on the other side too.
        let first = y_lo.floor().clamp(0.0, max_row) as usize;
        let last = (y_hi.ceil() - 1.0).max(y_lo.floor()).clamp(0.0, max_row) as usize;
        let max_col = self.width as f64 - 1.0;
        for y in first..=last {
            let (s0, s1) = (y as f64, y as f64 + 1.0);
            let (mut x_lo, mut x_hi) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut take = |x: f64| {
                x_lo = x_lo.min(x);
                x_hi = x_hi.max(x);
            };
            for (i, p) in poly.iter().enumerate() {
                if p[1] >= s0 && p[1] <= s1 {
                    take(p[0]);
                }
                let q = poly[(i + 1) % poly.len()];
                let (lo, hi) = (p[1].min(q[1]), p[1].max(q[1]));
                if hi > lo {
                    for s in [s0, s1] {
                        if s > lo && s < hi {
                            take(p[0] + (q[0] - p[0]) * (s - p[1]) / (q[1] - p[1]));
                        }
                    }
                }
            }
            if x_lo > x_hi || x_hi < 0.0 || x_lo > max_col + 1.0 {
                continue;
            }
            let x0 = x_lo.floor().clamp(0.0, max_col) as usize;
            let x1 = (x_hi.ceil() - 1.0).max(x_lo.floor()).clamp(0.0, max_col) as usize;
            self.set_span(y, x0, x1);
        }
    }

    /// Sets every cell `other`, of the same size, has set.
    pub(crate) fn merge(&mut self, other: &BitGrid) {
        for (a, b) in self.bits.iter_mut().zip(&other.bits) {
            *a |= b;
        }
    }

    /// The runs of set cells of row `y`, as `[start, end)`.
    pub(crate) fn runs(&self, y: usize) -> Vec<(usize, usize)> {
        runs_of(self.row(y), self.width, false)
    }

    /// Sets every cell that the unset cells along the border cannot reach
    /// through unset cells sharing an edge: the inside of every closed
    /// outline. Cross-sections come out as outlines; this makes them solid.
    pub(crate) fn fill_holes(&mut self) {
        let h = self.height;
        if h == 0 {
            return;
        }
        let free: Vec<Vec<(usize, usize)>> = (0..h)
            .map(|y| runs_of(self.row(y), self.width, true))
            .collect();
        let mut outside: Vec<Vec<bool>> = free.iter().map(|r| vec![false; r.len()]).collect();
        let mut queue: Vec<(usize, usize)> = Vec::new();
        for (y, spans) in free.iter().enumerate() {
            for (i, &(a, b)) in spans.iter().enumerate() {
                if y == 0 || y == h - 1 || a == 0 || b == self.width {
                    outside[y][i] = true;
                    queue.push((y, i));
                }
            }
        }
        while let Some((y, i)) = queue.pop() {
            let (a, b) = free[y][i];
            for ny in [y.wrapping_sub(1), y + 1] {
                if ny >= h {
                    continue;
                }
                for (j, &(c, d)) in free[ny].iter().enumerate() {
                    if c >= b {
                        break;
                    }
                    if d > a && !outside[ny][j] {
                        outside[ny][j] = true;
                        queue.push((ny, j));
                    }
                }
            }
        }
        for (y, spans) in free.iter().enumerate() {
            for (i, &(a, b)) in spans.iter().enumerate() {
                if !outside[y][i] {
                    set_bits(self.row_mut(y), a, b);
                }
            }
        }
    }

    /// This grid grown by `radius` cells: a cell is set when some set cell
    /// lies closer than `radius` cells, measured between the two cells'
    /// nearest points. So the grown grid covers everything within `radius`
    /// cells of what this one covers. The caller leaves room at the edges.
    pub(crate) fn grown(&self, radius: f64) -> BitGrid {
        let mut out = BitGrid::new(self.width, self.height);
        if radius <= 0.0 {
            out.bits.clone_from(&self.bits);
            return out;
        }
        // Half-widths of the kernel, row by row: two cells `dx`, `dy` apart
        // are `max(dx - 1, 0)`, `max(dy - 1, 0)` apart at their nearest.
        let widths: Vec<usize> = (0..)
            .map_while(|dy: usize| {
                let gy = dy.saturating_sub(1) as f64;
                let room = radius * radius - gy * gy;
                // The cells with (dx - 1)^2 < room; at least the one above.
                (room > 0.0).then(|| (room.sqrt() + 1.0 - 1e-9).floor() as usize)
            })
            .collect();
        let mut wide = vec![0u64; self.words];
        for y in 0..self.height {
            let src = self.row(y);
            if src.iter().all(|&w| w == 0) {
                continue;
            }
            for (dy, &w) in widths.iter().enumerate() {
                widen(src, &mut wide, w);
                for ny in [y as isize - dy as isize, y as isize + dy as isize] {
                    if ny < 0 || ny as usize >= self.height {
                        continue;
                    }
                    for (d, s) in out.row_mut(ny as usize).iter_mut().zip(&wide) {
                        *d |= s;
                    }
                    if dy == 0 {
                        break;
                    }
                }
            }
        }
        out.mask_tail();
        out
    }

    /// Clears the bits past `width` in every row.
    fn mask_tail(&mut self) {
        let tail = self.width % 64;
        if tail == 0 {
            return;
        }
        let keep = (1u64 << tail) - 1;
        let words = self.words;
        for y in 0..self.height {
            self.bits[y * words + words - 1] &= keep;
        }
    }

    /// The number of set cells.
    pub(crate) fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// The set cells' bounds, `[x0, y0, x1, y1)`, or `None` for an empty grid.
    pub(crate) fn bounds(&self) -> Option<[usize; 4]> {
        let mut b: Option<[usize; 4]> = None;
        for y in 0..self.height {
            let row = self.row(y);
            let Some(first) = row.iter().position(|&w| w != 0) else {
                continue;
            };
            let last = row.iter().rposition(|&w| w != 0).unwrap_or(first);
            let x0 = first * 64 + row[first].trailing_zeros() as usize;
            let x1 = last * 64 + 64 - row[last].leading_zeros() as usize;
            b = Some(match b {
                None => [x0, y, x1, y + 1],
                Some([a, c, d, _]) => [a.min(x0), c, d.max(x1), y + 1],
            });
        }
        b
    }
}

/// Sets bits `x0..x1` of a row.
fn set_bits(row: &mut [u64], x0: usize, x1: usize) {
    if x0 >= x1 {
        return;
    }
    let (w0, w1) = (x0 / 64, (x1 - 1) / 64);
    let lo = !0u64 << (x0 % 64);
    let hi = !0u64 >> (63 - (x1 - 1) % 64);
    if w0 == w1 {
        row[w0] |= lo & hi;
        return;
    }
    row[w0] |= lo;
    for w in &mut row[w0 + 1..w1] {
        *w = !0;
    }
    row[w1] |= hi;
}

/// `dst` = `src` with every set bit spread `w` bits to each side.
fn widen(src: &[u64], dst: &mut [u64], w: usize) {
    dst.copy_from_slice(src);
    for k in 1..=w {
        let (q, s) = (k / 64, k % 64);
        for i in 0..src.len() {
            // Shift towards higher bits by k.
            let up = if i >= q {
                let a = src[i - q] << s;
                let b = if s > 0 && i > q {
                    src[i - q - 1] >> (64 - s)
                } else {
                    0
                };
                a | b
            } else {
                0
            };
            // And towards lower bits.
            let down = if i + q < src.len() {
                let a = src[i + q] >> s;
                let b = if s > 0 && i + q + 1 < src.len() {
                    src[i + q + 1] << (64 - s)
                } else {
                    0
                };
                a | b
            } else {
                0
            };
            dst[i] |= up | down;
        }
    }
}

/// The runs of set bits (or of unset bits, `free`) of a row `width` cells
/// wide, as `[start, end)`.
fn runs_of(row: &[u64], width: usize, free: bool) -> Vec<(usize, usize)> {
    let bit = |x: usize| (row[x / 64] >> (x % 64) & 1 == 1) != free;
    let mut runs = Vec::new();
    let mut x = 0;
    while x < width {
        // Skip whole words with nothing of interest.
        if x % 64 == 0 {
            let w = if free { !row[x / 64] } else { row[x / 64] };
            if w == 0 {
                x += 64;
                continue;
            }
        }
        if !bit(x) {
            x += 1;
            continue;
        }
        let start = x;
        while x < width && bit(x) {
            if x % 64 == 0 {
                let w = if free { !row[x / 64] } else { row[x / 64] };
                if w == !0 && x + 64 <= width {
                    x += 64;
                    continue;
                }
            }
            x += 1;
        }
        runs.push((start, x));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_triangle_sets_every_cell_it_touches() {
        let mut g = BitGrid::new(10, 10);
        g.fill_convex(&[[1.0, 1.0], [5.0, 1.0], [1.0, 5.0]]);
        // The corner cell, the hypotenuse's cells, nothing past it.
        assert!(g.get(1, 1) && g.get(4, 1) && g.get(1, 4));
        assert!(g.get(2, 3), "on the hypotenuse");
        assert!(!g.get(4, 4) && !g.get(0, 0) && !g.get(6, 1));
    }

    #[test]
    fn a_wall_seen_edge_on_is_a_line() {
        let mut g = BitGrid::new(10, 10);
        g.fill_convex(&[[2.5, 2.5], [2.5, 7.5], [2.5, 7.5]]);
        assert_eq!(g.count(), 6);
        assert!((2..=7).all(|y| g.get(2, y)));
    }

    #[test]
    fn an_outline_comes_out_solid() {
        let mut g = BitGrid::new(12, 12);
        for x in 2..10 {
            g.set_span(2, x, x);
            g.set_span(9, x, x);
        }
        for y in 2..10 {
            g.set_span(y, 2, 2);
            g.set_span(y, 9, 9);
        }
        // An open ring elsewhere stays open.
        g.fill_holes();
        assert!(g.get(5, 5) && g.get(3, 8));
        assert!(!g.get(0, 0) && !g.get(10, 5));
        assert_eq!(g.count(), 64);
        let mut open = BitGrid::new(12, 12);
        for x in 2..10 {
            open.set_span(2, x, x);
        }
        open.fill_holes();
        assert_eq!(open.count(), 8);
    }

    #[test]
    fn growing_covers_the_reach_and_no_more() {
        let mut g = BitGrid::new(140, 9);
        g.set_span(4, 70, 70);
        let grown = g.grown(2.0);
        // Two cells apart: within reach of the cell's edge.
        assert!(grown.get(72, 4) && grown.get(68, 4) && grown.get(70, 6));
        assert!(grown.get(72, 6), "a corner two cells off is 1.4 away");
        assert!(!grown.get(73, 4) && !grown.get(70, 7));
        // Across a word boundary.
        let mut g = BitGrid::new(140, 3);
        g.set_span(1, 63, 64);
        let grown = g.grown(1.0);
        assert!(grown.get(62, 1) && grown.get(65, 1) && !grown.get(66, 1));
        assert_eq!(grown.bounds(), Some([62, 0, 66, 3]));
    }

    #[test]
    fn runs_and_bounds() {
        let mut g = BitGrid::new(200, 2);
        g.set_span(0, 3, 5);
        g.set_span(0, 60, 130);
        assert_eq!(g.runs(0), vec![(3, 6), (60, 131)]);
        assert_eq!(g.bounds(), Some([3, 0, 131, 1]));
        assert_eq!(BitGrid::new(5, 5).bounds(), None);
    }
}
