/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The perimeter of a layer from its pixels (#240, #94).
//!
//! Counting the edges between lit and dark pixels inflates every edge that
//! is not along a row or a column: a diagonal becomes a stair, `√2` times
//! too long. The Cauchy–Crofton formula measures a curve by how often lines
//! cross it instead:
//!
//! ```text
//! L = ½ ∫₀^π ∫ n(θ, p) dp dθ
//! ```
//!
//! where `n(θ, p)` counts the crossings of the line at angle `θ` and offset
//! `p`. On a pixel grid, the lines through pixel centres along eight
//! directions (rows, columns, the two diagonals and the four knight moves)
//! give `n` as the number of neighbour pairs that differ, each family
//! spaced by the cell area over the length of its step. Each direction
//! weighs the angle around it. On straight edges at any angle the estimate
//! stays within about 2 % of the true length, on rectangular pixels too.
//!
//! Pixels are counted whole: a pixel is lit or it is not. What lies outside
//! the rows given is dark.

/// The eight steps, `(dx, dy)` in pixels, the lines run along.
const STEPS: [(i64, i64); 8] = [
    (1, 0),
    (0, 1),
    (1, 1),
    (-1, 1),
    (2, 1),
    (-2, 1),
    (1, 2),
    (-1, 2),
];

/// Measures the perimeter of a layer given row by row.
#[derive(Clone, Debug)]
pub struct PerimeterMeter {
    /// `½ · weight · spacing` of each step's family of lines, in mm.
    scale: [f64; 8],
    /// The last row given, normalised.
    prev: Vec<(i64, i64)>,
    /// The row before it.
    prev2: Vec<(i64, i64)>,
    /// The index of the row after the last one counted.
    next_row: u64,
    /// The row being given, not counted yet.
    pending_row: Option<u64>,
    /// Its spans so far.
    pending: Vec<(i64, i64)>,
    /// The neighbour pairs that differ, along each step.
    crossings: [u64; 8],
}

impl PerimeterMeter {
    /// A meter for pixels `pixel_mm[0]` wide (along a row) and `pixel_mm[1]`
    /// tall.
    pub fn new(pixel_mm: [f64; 2]) -> Self {
        let [px, py] = pixel_mm.map(|v| if v.is_finite() && v > 0.0 { v } else { 1.0 });
        let angles: [f64; 8] = STEPS.map(|(dx, dy)| (dy as f64 * py).atan2(dx as f64 * px));
        let mut order: Vec<usize> = (0..8).collect();
        order.sort_by(|&a, &b| angles[a].total_cmp(&angles[b]));
        let mut scale = [0.0; 8];
        for (k, &i) in order.iter().enumerate() {
            let before = if k == 0 {
                angles[order[7]] - std::f64::consts::PI
            } else {
                angles[order[k - 1]]
            };
            let after = if k == 7 {
                angles[order[0]] + std::f64::consts::PI
            } else {
                angles[order[k + 1]]
            };
            let weight = 0.5 * (after - before);
            let (dx, dy) = STEPS[i];
            let spacing = px * py / (dx as f64 * px).hypot(dy as f64 * py);
            scale[i] = 0.5 * weight * spacing;
        }
        Self {
            scale,
            prev: Vec::new(),
            prev2: Vec::new(),
            next_row: 0,
            pending_row: None,
            pending: Vec::new(),
            crossings: [0; 8],
        }
    }

    /// Adds row `row`: its lit pixels are the half-open spans `x0..x1` of
    /// `spans`, in any order, overlapping or not. Rows come in increasing
    /// order; a row skipped is dark, and a row given again adds to it.
    ///
    /// # Panics
    ///
    /// In debug builds, when `row` comes before a row already given.
    pub fn push_row(&mut self, row: u32, spans: impl IntoIterator<Item = (u32, u32)>) {
        let row = u64::from(row);
        let spans = spans
            .into_iter()
            .filter(|(x0, x1)| x1 > x0)
            .map(|(x0, x1)| (i64::from(x0), i64::from(x1)));
        if self.pending_row == Some(row) {
            self.pending.extend(spans);
            return;
        }
        self.flush();
        debug_assert!(row >= self.next_row, "rows out of order");
        // Past two dark rows, more of them change nothing.
        for _ in 0..row.saturating_sub(self.next_row).min(2) {
            self.advance(Vec::new());
        }
        self.next_row = row;
        self.pending_row = Some(row);
        self.pending.extend(spans);
    }

    /// The perimeter of everything given, in millimetres.
    pub fn finish(mut self) -> f64 {
        self.flush();
        self.advance(Vec::new());
        self.advance(Vec::new());
        self.crossings
            .iter()
            .zip(self.scale)
            .map(|(&count, scale)| count as f64 * scale)
            .sum()
    }

    fn flush(&mut self) {
        if self.pending_row.take().is_some() {
            let row = normalised(std::mem::take(&mut self.pending));
            self.advance(row);
        }
    }

    fn advance(&mut self, row: Vec<(i64, i64)>) {
        self.crossings[0] += 2 * row.len() as u64;
        for (i, &(dx, dy)) in STEPS.iter().enumerate().skip(1) {
            let other = if dy == 1 { &self.prev } else { &self.prev2 };
            self.crossings[i] += symmetric_difference(other, dx, &row);
        }
        self.prev2 = std::mem::replace(&mut self.prev, row);
        self.next_row += 1;
    }
}

/// The perimeter of the pixels `runs` lights, `(row, x0, x1)` each, rows in
/// increasing order, on pixels of `pixel_mm`.
pub fn perimeter_of_rows(
    pixel_mm: [f64; 2],
    runs: impl IntoIterator<Item = (u32, u32, u32)>,
) -> f64 {
    let mut meter = PerimeterMeter::new(pixel_mm);
    let mut row: Option<(u32, Vec<(u32, u32)>)> = None;
    for (y, x0, x1) in runs {
        match &mut row {
            Some((current, spans)) if *current == y => spans.push((x0, x1)),
            _ => {
                if let Some((current, spans)) = row.take() {
                    meter.push_row(current, spans);
                }
                row = Some((y, vec![(x0, x1)]));
            }
        }
    }
    if let Some((current, spans)) = row {
        meter.push_row(current, spans);
    }
    meter.finish()
}

/// `spans` sorted, with those that overlap or touch merged.
fn normalised(mut spans: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
    for (x0, x1) in spans {
        match out.last_mut() {
            Some(last) if x0 <= last.1 => last.1 = last.1.max(x1),
            _ => out.push((x0, x1)),
        }
    }
    out
}

/// The pixels lit in exactly one of `a` moved by `dx` and `b`.
fn symmetric_difference(a: &[(i64, i64)], dx: i64, b: &[(i64, i64)]) -> u64 {
    let len = |spans: &[(i64, i64)]| spans.iter().map(|(x0, x1)| x1 - x0).sum::<i64>();
    let (mut i, mut j, mut shared) = (0, 0, 0i64);
    while i < a.len() && j < b.len() {
        let (a0, a1) = (a[i].0 + dx, a[i].1 + dx);
        let (b0, b1) = b[j];
        shared += (a1.min(b1) - a0.max(b0)).max(0);
        if a1 < b1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    (len(a) + len(b) - 2 * shared) as u64
}

#[cfg(test)]
#[path = "tests_perimeter.rs"]
mod tests_perimeter;
