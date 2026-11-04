/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The solid of one role on sample lines, exact along each line, and its
//! opening by a round disc: the measuring tools of the thin-feature check.
//!
//! A line is a row of the measuring grid, or a column for
//! [`Axis::Columns`]. Along a line the solid is a list of *stretches* whose
//! ends are the exact crossings of the contours; across the lines it is
//! sampled. A disc is cut the same way: on every line it reaches, a stretch
//! of its exact half-width there. So a width along the lines is measured
//! exactly, and a width across them to the spacing of the lines.
//!
//! Coordinates are in sub-pixels of the check's grid. A line is numbered:
//! line `i` lies at `origin + i * step` across.

use std::cell::OnceCell;

use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::runs::Run;

/// Ends closer than this, in sub-pixels, are the same point.
pub(super) const EPS: f64 = 1e-7;

/// Which way the lines run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    /// Lines are rows: along is X, across is Y.
    Rows,
    /// Lines are columns: along is Y, across is X.
    Columns,
}

/// A contour edge in line space: `y` across the lines, `x` along them.
#[derive(Clone, Copy, Debug)]
struct Edge {
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    /// How far `x` moves per unit of `y`.
    slope: f64,
}

impl Edge {
    /// The lines it can cross: `lo <= y < hi`.
    fn span(&self) -> (f64, f64) {
        (self.ay.min(self.by), self.ay.max(self.by))
    }

    /// `+1` going up across the lines, `-1` going down.
    fn direction(&self) -> i32 {
        if self.ay < self.by { 1 } else { -1 }
    }

    /// Where it crosses the line at `y`, with `+1` going up across the lines
    /// and `-1` going down, if it does.
    fn crossing(&self, y: f64) -> Option<(f64, i32)> {
        ((self.ay > y) != (self.by > y)).then(|| {
            let x = (y - self.ay).mul_add(self.slope, self.ax);
            (x, self.direction())
        })
    }
}

/// The edges of one role on one layer, to cut lines across them: in order,
/// sweeping consecutive lines, or one line anywhere.
#[derive(Clone, Debug)]
pub(super) struct Edges {
    /// The edges that reach the grid, sorted by the first line they can
    /// cross.
    edges: Vec<Edge>,
    /// The extent of the grid along the lines: stretches are clipped to it.
    along: f64,
    /// The extent of the grid across the lines: lines outside it are empty.
    across: f64,
    /// The edges sorted into bands across the lines, built on the first cut
    /// of a single line.
    bands: OnceCell<Bands>,
}

/// The edges by band across the lines, to cut one line looking only at the
/// edges near the stretch of it wanted. In each band, the edges that cross
/// it from side to side are sorted by how far along they reach, with the
/// running sum of their directions: those that end before the stretch cross
/// any line of the band before it, and only their sum matters. The others,
/// which end inside the band, are few and are tested one by one.
#[derive(Clone, Debug)]
struct Bands {
    /// Start of each band's entries in `through`, plus the end.
    through_starts: Vec<usize>,
    /// The edges crossing a band from side to side: how far along each
    /// reaches, and its index, sorted.
    through: Vec<(f64, u32)>,
    /// For every entry of `through`, the sum of the directions of the
    /// entries before it in its band.
    before: Vec<i32>,
    /// For every band, how far along the longest edge of its `through`
    /// reaches back from its far end.
    extent: Vec<f64>,
    /// Start of each band's entries in `partial`, plus the end.
    partial_starts: Vec<usize>,
    /// The edges ending inside a band.
    partial: Vec<u32>,
}

/// The width of a band of [`Bands`], in sub-pixels.
const BAND: f64 = 4.0;

impl Edges {
    /// The edges of `contours` on the screen `map`, in sub-pixels of a grid
    /// `scale` times finer and `size` (width, height) sub-pixels large, laid
    /// out for lines along `axis`.
    pub(super) fn new(
        contours: &[&ContourLayer],
        map: &PixelMap,
        scale: f64,
        axis: Axis,
        size: [u32; 2],
    ) -> Self {
        let _clock = crate::clock::scope("lines.edges_new");
        let [width, height] = size.map(f64::from);
        let (along, across) = match axis {
            Axis::Rows => (width, height),
            Axis::Columns => (height, width),
        };
        let mut edges = Vec::new();
        let mut points: Vec<(f64, f64)> = Vec::new();
        for contour in contours {
            for polygon in contour.polygons() {
                for ring in std::iter::once(polygon.outer()).chain(polygon.holes()) {
                    points.clear();
                    points.extend(ring.vertices().iter().map(|&v| {
                        let (x, y) = map.mm_to_pixel(v);
                        match axis {
                            Axis::Rows => (x * scale, y * scale),
                            Axis::Columns => (y * scale, x * scale),
                        }
                    }));
                    let n = points.len();
                    if n < 2 {
                        continue;
                    }
                    for i in 0..n {
                        let (ax, ay) = points[i];
                        let (bx, by) = points[(i + 1) % n];
                        let edge = Edge {
                            ax,
                            ay,
                            bx,
                            by,
                            slope: (bx - ax) / (by - ay),
                        };
                        let (lo, hi) = edge.span();
                        // An edge along a line never crosses one, nor does
                        // one off the grid.
                        if lo < hi && hi >= 0.0 && lo < across && edge.slope.is_finite() {
                            edges.push(edge);
                        }
                    }
                }
            }
        }
        edges.sort_unstable_by(|a, b| a.span().0.total_cmp(&b.span().0));
        crate::clock::count("items.thin_edges", edges.len() as u64);
        Self {
            edges,
            along,
            across,
            bands: OnceCell::new(),
        }
    }

    /// The bands, built on first use.
    fn bands(&self) -> &Bands {
        self.bands.get_or_init(|| {
            let _clock = crate::clock::scope("lines.bands");
            let count = (self.across / BAND).ceil().max(1.0) as usize;
            let mut through: Vec<Vec<(f64, u32)>> = vec![Vec::new(); count];
            let mut partial: Vec<Vec<u32>> = vec![Vec::new(); count];
            for (index, e) in self.edges.iter().enumerate() {
                let (lo, hi) = e.span();
                let first = (lo / BAND).floor().max(0.0) as usize;
                let last = ((hi / BAND).floor().max(0.0) as usize).min(count - 1);
                let far = e.ax.max(e.bx);
                for band in first..=last {
                    let side = band as f64 * BAND;
                    if lo <= side && hi >= side + BAND {
                        through[band].push((far, index as u32));
                    } else {
                        partial[band].push(index as u32);
                    }
                }
            }
            let mut bands = Bands {
                through_starts: vec![0],
                through: Vec::new(),
                before: Vec::new(),
                extent: Vec::with_capacity(count),
                partial_starts: vec![0],
                partial: Vec::new(),
            };
            for (mut band, edges) in through.into_iter().zip(partial) {
                band.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let mut sum = 0;
                let mut extent = 0.0f64;
                for &(far, index) in &band {
                    let e = &self.edges[index as usize];
                    bands.before.push(sum);
                    sum += e.direction();
                    extent = extent.max(far - e.ax.min(e.bx));
                }
                bands.through.extend(band);
                bands.through_starts.push(bands.through.len());
                bands.extent.push(extent);
                bands.partial.extend(edges);
                bands.partial_starts.push(bands.partial.len());
            }
            bands
        })
    }

    /// Pushes onto `out`, empty, the stretches where the solid covers the
    /// line at `y` across, between `lo` and `hi` along and within the grid.
    /// Only the edges near that stretch are looked at. `crossings` is
    /// scratch space.
    fn cut(
        &self,
        y: f64,
        lo: f64,
        hi: f64,
        crossings: &mut Vec<(f64, i32)>,
        out: &mut Vec<(f64, f64)>,
    ) {
        crossings.clear();
        if !(0.0..self.across).contains(&y) {
            return;
        }
        let bands = self.bands();
        let band = ((y / BAND) as usize).min(bands.extent.len() - 1);
        // The winding just before `lo`.
        let mut winding = 0;
        let (from, to) = (bands.through_starts[band], bands.through_starts[band + 1]);
        let through = &bands.through[from..to];
        let ended = through.partition_point(|&(far, _)| far < lo);
        if ended > 0 {
            let (_, last) = through[ended - 1];
            winding = bands.before[from + ended - 1] + self.edges[last as usize].direction();
        }
        let reach = hi + bands.extent[band];
        let near = through[ended..]
            .iter()
            .take_while(|&&(far, _)| far <= reach)
            .map(|&(_, index)| index);
        let (from, to) = (bands.partial_starts[band], bands.partial_starts[band + 1]);
        for index in near.chain(bands.partial[from..to].iter().copied()) {
            if let Some((x, dir)) = self.edges[index as usize].crossing(y) {
                if x < lo {
                    winding += dir;
                } else if x <= hi {
                    crossings.push((x, dir));
                }
            }
        }
        self.stretches(crossings, winding, lo, hi, out);
    }

    /// Pushes onto `out` the stretches where the winding is not zero, from
    /// `winding` at `lo` and through `crossings`, clipped to `lo..=hi` along
    /// and to the grid.
    fn stretches(
        &self,
        crossings: &mut [(f64, i32)],
        mut winding: i32,
        lo: f64,
        hi: f64,
        out: &mut Vec<(f64, f64)>,
    ) {
        crossings.sort_unstable_by(|p, q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)));
        let (lo, hi) = (lo.max(0.0), hi.min(self.along));
        let mut start = lo;
        for &(x, dir) in crossings.iter() {
            let was = winding != 0;
            winding += dir;
            if !was && winding != 0 {
                start = x;
            } else if was && winding == 0 {
                let (x0, x1) = (start.max(lo), x.min(hi));
                if x0 < x1 {
                    push_merged(out, x0, x1);
                }
            }
        }
        if winding != 0 && start < hi {
            push_merged(out, start.max(lo), hi);
        }
    }

    /// The solid on `count` lines from line `first`, line `i` lying at
    /// `origin + i * step` across (`step` > 0), along the whole grid. The
    /// lines are swept in order, each crossed only by the edges reaching it.
    pub(super) fn lines(&self, origin: f64, step: f64, first: i64, count: usize) -> Lines {
        let _clock = crate::clock::scope("lines.lines");
        let mut lines = Lines::starting_at(first);
        let (mut crossings, mut line) = (Vec::new(), Vec::new());
        let mut active: Vec<u32> = Vec::new();
        let mut next = 0;
        for i in 0..count as i64 {
            let y = (first + i) as f64 * step + origin;
            while next < self.edges.len() && self.edges[next].span().0 <= y {
                active.push(next as u32);
                next += 1;
            }
            active.retain(|&e| self.edges[e as usize].span().1 > y);
            crossings.clear();
            crossings.extend(
                active
                    .iter()
                    .filter_map(|&e| self.edges[e as usize].crossing(y)),
            );
            line.clear();
            self.stretches(&mut crossings, 0, 0.0, self.along, &mut line);
            lines.push_line(&line);
        }
        lines
    }

    /// The stretches of the line at `y` across between `lo` and `hi` along.
    #[cfg(test)]
    pub(super) fn cut_line(&self, y: f64, lo: f64, hi: f64) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        self.cut(y, lo, hi, &mut Vec::new(), &mut out);
        out
    }

    /// The solid around the point `along`, `across`, to measure the local
    /// thickness there: lines `step` apart through it and up to `reach` on
    /// either side, line 0 through the point, clipped to `reach` on either
    /// side along.
    pub(super) fn window(&self, along: f64, across: f64, reach: f64, step: f64) -> Window<'_> {
        let half = (reach / step).ceil() as i64;
        let mut lines = Lines::starting_at(-half);
        let (mut crossings, mut line) = (Vec::new(), Vec::new());
        for i in -half..=half {
            let y = (i as f64).mul_add(step, across);
            line.clear();
            self.cut(y, along - reach, along + reach, &mut crossings, &mut line);
            lines.push_line(&line);
        }
        Window {
            edges: self,
            lines,
            along,
            across,
            reach,
            step,
        }
    }
}

/// Appends `[x0, x1]` to stretches sorted by start, merging it with the last
/// one when they overlap or touch.
fn push_merged(out: &mut Vec<(f64, f64)>, x0: f64, x1: f64) {
    match out.last_mut() {
        Some(last) if x0 <= last.1 + EPS => last.1 = last.1.max(x1),
        _ => out.push((x0, x1)),
    }
}

/// Solid on consecutive lines: on each, closed stretches `[x0, x1]` sorted
/// along the line and apart from each other. A stretch may be a single point
/// (`x0 == x1`): where a disc exactly as wide as the solid fits.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Lines {
    first: i64,
    /// Start of each line's stretches, plus the end.
    starts: Vec<usize>,
    stretches: Vec<(f64, f64)>,
}

impl Lines {
    /// No line yet; the first pushed is line `first`.
    pub(super) fn starting_at(first: i64) -> Self {
        Self {
            first,
            starts: vec![0],
            stretches: Vec::new(),
        }
    }

    /// Appends the next line.
    pub(super) fn push_line(&mut self, stretches: &[(f64, f64)]) {
        self.stretches.extend_from_slice(stretches);
        self.starts.push(self.stretches.len());
    }

    /// The stretches of line `i`: none outside the lines held.
    pub(super) fn line(&self, i: i64) -> &[(f64, f64)] {
        let k = i - self.first;
        if k < 0 || k as usize + 1 >= self.starts.len() {
            return &[];
        }
        &self.stretches[self.starts[k as usize]..self.starts[k as usize + 1]]
    }

    /// The stretches within `region`, a run list whose rows are these lines:
    /// each run `x0..x1` of row `i` keeps what lies between `x0` and `x1` on
    /// line `i`.
    pub(super) fn clipped_to(&self, region: &[Run]) -> Self {
        let (Some(first), Some(last)) = (region.first(), region.last()) else {
            return Self::default();
        };
        let mut out = Self::starting_at(i64::from(first.row));
        let mut line = Vec::new();
        let mut at = 0;
        for row in first.row..=last.row {
            line.clear();
            let stretches = self.line(i64::from(row));
            let mut j = 0;
            while at < region.len() && region[at].row == row {
                let (lo, hi) = (f64::from(region[at].x0), f64::from(region[at].x1));
                while j < stretches.len() && stretches[j].1 < lo {
                    j += 1;
                }
                for &(a, b) in stretches[j..].iter().take_while(|s| s.0 <= hi) {
                    let (a, b) = (a.max(lo), b.min(hi));
                    if a < b {
                        line.push((a, b));
                    }
                }
                at += 1;
            }
            out.push_line(&line);
        }
        out
    }

    /// The first and last line holding a stretch.
    fn span(&self) -> Option<(i64, i64)> {
        let count = self.starts.len().saturating_sub(1);
        let first = (0..count).find(|&k| self.starts[k + 1] > self.starts[k])?;
        let last = (0..count)
            .rev()
            .find(|&k| self.starts[k + 1] > self.starts[k])?;
        Some((self.first + first as i64, self.first + last as i64))
    }

    /// The sub-pixels whose centres the stretches cover, line `i` being
    /// row `i` through the centres of a grid `width` sub-pixels wide.
    /// `closed`: a centre on an end counts (an opening); otherwise the
    /// stretch is half-open, `[x0, x1)`, as the printer's raster fills a
    /// solid.
    pub(super) fn cells(&self, width: u32, closed: bool) -> Vec<Run> {
        let _clock = crate::clock::scope("lines.cells");
        let mut runs: Vec<Run> = Vec::new();
        let limit = f64::from(width);
        for k in 0..self.starts.len().saturating_sub(1) {
            let row = self.first + k as i64;
            if row < 0 || row > i64::from(u32::MAX) {
                continue;
            }
            for &(x0, x1) in &self.stretches[self.starts[k]..self.starts[k + 1]] {
                let (from, to) = if closed {
                    ((x0 - EPS - 0.5).ceil(), (x1 + EPS - 0.5).floor() + 1.0)
                } else {
                    ((x0 - 0.5).ceil(), (x1 - 0.5).ceil())
                };
                let (from, to) = (from.max(0.0), to.min(limit));
                if from >= to {
                    continue;
                }
                let run = Run::new(row as u32, from as u32, to as u32);
                match runs.last_mut() {
                    Some(last) if last.row == run.row && run.x0 <= last.x1 => {
                        last.x1 = last.x1.max(run.x1);
                    }
                    _ => runs.push(run),
                }
            }
        }
        runs
    }
}

/// A round disc cut on lines: on each line it reaches, the half-width of the
/// stretch it covers. The disc is centred on a line.
#[derive(Clone, Debug)]
pub(super) struct Disc {
    /// `(line offset, half-width)`, widest first.
    rows: Vec<(i64, f64)>,
    /// The farthest line offset reached.
    reach: i64,
}

impl Disc {
    /// A disc of `radius` on lines `step` apart. It reaches every line up to
    /// `radius` away, and one more line beyond, with a half-width of 0, when
    /// `radius` falls between lines: across the lines it then needs more
    /// room than the round disc, never less.
    pub(super) fn new(radius: f64, step: f64) -> Self {
        Self::within(radius, step, (radius / step - 1e-9).ceil().max(0.0) as i64)
    }

    /// A disc of `radius` on lines `step` apart, cut only on the lines up to
    /// `reach` away, all within the radius.
    fn within(radius: f64, step: f64, reach: i64) -> Self {
        let mut rows = vec![(0, radius)];
        for k in 1..=reach {
            let d = k as f64 * step;
            let half = radius.mul_add(radius, -d * d).max(0.0).sqrt();
            rows.push((k, half));
            rows.push((-k, half));
        }
        Self { rows, reach }
    }

    /// The farthest line offset the disc reaches.
    #[cfg(test)]
    pub(super) fn reach(&self) -> i64 {
        self.reach
    }

    /// The half-width on line offset `k`, if the disc reaches it.
    fn half_width(&self, k: i64) -> Option<f64> {
        let at = match k {
            0 => 0,
            k if k > 0 => 2 * k as usize - 1,
            k => 2 * (-k) as usize,
        };
        self.rows.get(at).map(|&(_, half)| half)
    }
}

/// Erosion: on each line, the centres where `disc` fits in `lines`.
pub(super) fn erode(lines: &Lines, disc: &Disc) -> Lines {
    let Some((lo, hi)) = lines.span() else {
        return Lines::default();
    };
    let mut out = Lines::starting_at(lo + disc.reach);
    let mut acc = Vec::new();
    let mut next = Vec::new();
    for anchor in (lo + disc.reach)..=(hi - disc.reach) {
        acc.clear();
        for (i, &(k, half)) in disc.rows.iter().enumerate() {
            let line = lines.line(anchor + k);
            if i == 0 {
                shrink_into(line, half, &mut acc);
            } else {
                shrink_and_intersect(&acc, line, half, &mut next);
                std::mem::swap(&mut acc, &mut next);
            }
            if acc.is_empty() {
                break;
            }
        }
        out.push_line(&acc);
    }
    out
}

/// Dilation: every point `disc` covers when centred on `centres`.
pub(super) fn dilate(centres: &Lines, disc: &Disc) -> Lines {
    let Some((lo, hi)) = centres.span() else {
        return Lines::default();
    };
    let mut out = Lines::starting_at(lo - disc.reach);
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for y in (lo - disc.reach)..=(hi + disc.reach) {
        spans.clear();
        for &(k, half) in &disc.rows {
            spans.extend(
                centres
                    .line(y - k)
                    .iter()
                    .map(|&(a, b)| (a - half, b + half)),
            );
        }
        spans.sort_unstable_by(|p, q| p.0.total_cmp(&q.0));
        merged.clear();
        for &(a, b) in &spans {
            push_merged(&mut merged, a, b);
        }
        out.push_line(&merged);
    }
    out
}

/// Opening: the union of every placement of `disc` that fits in `lines`.
pub(super) fn open(lines: &Lines, disc: &Disc) -> Lines {
    let _clock = crate::clock::scope("lines.open");
    dilate(&erode(lines, disc), disc)
}

/// The stretches of `line` shrunk by `half` at both ends, those still
/// holding a point.
fn shrink_into(line: &[(f64, f64)], half: f64, out: &mut Vec<(f64, f64)>) {
    out.extend(line.iter().filter_map(|&(a, b)| {
        let (a, b) = (a + half, b - half);
        (a <= b + EPS).then_some((a, b.max(a)))
    }));
}

/// The intersection of `acc` with the stretches of `line` shrunk by `half`.
fn shrink_and_intersect(
    acc: &[(f64, f64)],
    line: &[(f64, f64)],
    half: f64,
    out: &mut Vec<(f64, f64)>,
) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < acc.len() && j < line.len() {
        let (b0, b1) = (line[j].0 + half, line[j].1 - half);
        if b0 > b1 + EPS {
            j += 1;
            continue;
        }
        let lo = acc[i].0.max(b0);
        let hi = acc[i].1.min(b1);
        if lo <= hi + EPS {
            out.push((lo, hi.max(lo)));
        }
        if acc[i].1 <= b1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The solid around one point, cut on lines through it, to measure the
/// local thickness there.
pub(super) struct Window<'a> {
    edges: &'a Edges,
    lines: Lines,
    /// The point, along the lines.
    along: f64,
    /// The point, across the lines: line 0.
    across: f64,
    /// How far around the point the lines reach.
    reach: f64,
    /// The spacing of the lines.
    step: f64,
}

impl Window<'_> {
    /// The local thickness at the point: the diameter of the widest disc,
    /// up to `max`, that fits in the solid and covers the point, found by
    /// `steps` bisections. The disc is cut on the lines within its radius
    /// and, exactly, at its two ends across them, so it is never wider than
    /// the solid: the thickness is exact along the lines and errs short, by
    /// at most a line spacing, across them.
    pub(super) fn thickness(&self, max: f64, steps: u32) -> f64 {
        let mut scratch = Scratch::default();
        if self.covers(0.5 * max, &mut scratch) {
            return max;
        }
        let (mut lo, mut hi) = (0.0, 0.5 * max);
        for _ in 0..steps {
            let mid = 0.5 * (lo + hi);
            if self.covers(mid, &mut scratch) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        2.0 * lo
    }

    /// Whether a disc of `radius` fits in the solid somewhere it covers the
    /// point.
    fn covers(&self, radius: f64, scratch: &mut Scratch) -> bool {
        let reach = (radius / self.step + 1e-9).floor() as i64;
        let disc = Disc::within(radius, self.step, reach);
        // The ends of the disc across the lines, when they fall between
        // lines, are cut exactly.
        let ends = radius - reach as f64 * self.step > EPS;
        // Centres on the nearest lines first: a fit there ends the search.
        let anchors = std::iter::once(0).chain((1..=reach).flat_map(|k| [k, -k]));
        for anchor in anchors {
            // The disc centred on line `anchor` reaches line 0 with its row
            // `-anchor`: only centres that close to the point can cover it.
            let Some(half) = disc.half_width(-anchor) else {
                continue;
            };
            let acc = &mut scratch.acc;
            acc.clear();
            acc.push((self.along - half - EPS, self.along + half + EPS));
            let mut fits = true;
            for &(k, half) in &disc.rows {
                shrink_and_intersect(acc, self.lines.line(anchor + k), half, &mut scratch.next);
                std::mem::swap(acc, &mut scratch.next);
                if acc.is_empty() {
                    fits = false;
                    break;
                }
            }
            if fits && ends {
                let centre = (anchor as f64).mul_add(self.step, self.across);
                for y in [centre - radius, centre + radius] {
                    scratch.line.clear();
                    self.edges.cut(
                        y,
                        self.along - self.reach,
                        self.along + self.reach,
                        &mut scratch.crossings,
                        &mut scratch.line,
                    );
                    shrink_and_intersect(acc, &scratch.line, 0.0, &mut scratch.next);
                    std::mem::swap(acc, &mut scratch.next);
                    if acc.is_empty() {
                        fits = false;
                        break;
                    }
                }
            }
            if fits {
                return true;
            }
        }
        false
    }
}

/// Buffers reused by [`Window::covers`].
#[derive(Default)]
struct Scratch {
    acc: Vec<(f64, f64)>,
    next: Vec<(f64, f64)>,
    line: Vec<(f64, f64)>,
    crossings: Vec<(f64, i32)>,
}
