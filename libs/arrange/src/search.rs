/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where each body goes.
//!
//! One pass places the bodies one after the other, the largest first: each
//! at the angle and spot that keep the pile smallest (as the smallest copy
//! of the plate that holds it, so piles grow in the plate's shape and leave
//! the free room around them), pulled either towards
//! the plate's middle or into its front left corner, row by row ([`Pull`]).
//! A pass is quick, and its outcome swings with the order, so several passes
//! run side by side with the order shaken up a little, and the best one
//! wins: the most bodies on the plate, then the smallest pile, then, among
//! piles within a few percent of the smallest, the best peel balance. The
//! whole pile then shifts to balance the peel, which also centres a pile
//! made in the corner.
//!
//! A fill ([`fill`]) places copies one after the other in the same way,
//! until one no longer fits. Each copy of a shape looks for its spot from
//! where the last one found it: the plate only fills up, so the spots
//! before are still full, and a fill of a hundred copies costs about what
//! one pass over the plate does.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::footprint::{Footprint, Shape};
use crate::plate::Occupancy;

/// How far a pile may grow past the smallest and still win on balance.
const BALANCE_SLACK: f64 = 0.03;

/// How much a shaken-up pass may reorder the bodies: each one's size is
/// scaled by up to this much either way before sorting.
const SHAKE: f64 = 0.35;

/// The search's fixed inputs.
#[derive(Debug)]
pub(crate) struct Problem<'a> {
    pub(crate) footprints: &'a [Footprint],
    /// The plate with the bodies that stay where they are.
    pub(crate) plate: Occupancy,
    /// The plate cell bodies are pulled towards.
    pub(crate) target: [i32; 2],
    /// Plate cell size (mm) and the grid's corner (world mm).
    pub(crate) cell: f64,
    pub(crate) origin: [f64; 2],
    /// Whether the whole pile may shift to balance the peel: only when
    /// nothing stays put.
    pub(crate) balance: bool,
    pub(crate) passes: u32,
    pub(crate) deadline: Instant,
}

/// Where each body went in a pass.
#[derive(Clone, Debug)]
pub(crate) struct Layout {
    /// Per body, its angle and where its frame's corner sits (plate cells).
    pub(crate) placed: Vec<Option<(usize, [i32; 2])>>,
    pub(crate) count: usize,
    /// How much plate the pile needs ([`Metric::extent`]).
    pub(crate) extent: i64,
    /// The mean distance (mm) of each band's centroid from the plate's
    /// middle, weighted by its area, once the pile is shifted.
    pub(crate) imbalance: f64,
    /// How far the whole pile shifts (mm).
    pub(crate) shift: [f64; 2],
    pub(crate) pass: u32,
}

/// A small, seedable generator (xorshift64*): the passes are the same for
/// the same scene.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in [0, 1).
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Distances scaled to the plate: a step across the plate counts as much
/// as a step along it, so piles grow in the plate's shape.
#[derive(Clone, Copy, Debug)]
struct Metric {
    /// The plate's size (cells).
    size: [i64; 2],
}

impl Metric {
    fn dist2(&self, a: [i32; 2], b: [i32; 2]) -> i64 {
        let dx = i64::from(a[0] - b[0]) * self.size[1];
        let dy = i64::from(a[1] - b[1]) * self.size[0];
        dx * dx + dy * dy
    }

    /// How much plate a pile `[lo, hi)` needs: the smallest copy of the
    /// plate that holds it, as its size times the plate's (cells²).
    fn extent(&self, lo: [i32; 2], hi: [i32; 2]) -> i64 {
        let w = i64::from(hi[0] - lo[0]) * self.size[1];
        let h = i64::from(hi[1] - lo[1]) * self.size[0];
        w.max(h)
    }
}

/// Offsets on a grid of `step` cells within `reach` of the origin, nearest
/// first.
fn spiral(reach: [i32; 2], step: i32, metric: Metric) -> Vec<[i32; 2]> {
    let (nx, ny) = (reach[0] / step + 1, reach[1] / step + 1);
    let mut offsets: Vec<[i32; 2]> = (-ny..=ny)
        .flat_map(|j| (-nx..=nx).map(move |i| [i * step, j * step]))
        .collect();
    offsets.sort_by_key(|&[x, y]| (metric.dist2([x, y], [0, 0]), y, x));
    offsets
}

/// Spots on a grid of `step` cells over a plate `size` cells, front row
/// first, left to right.
fn rows(size: [i32; 2], step: i32) -> Vec<[i32; 2]> {
    (0..=size[1] / step)
        .flat_map(|j| (0..=size[0] / step).map(move |i| [i * step, j * step]))
        .collect()
}

/// Where a pass pulls the bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pull {
    /// As close to the plate's middle as they fit: a round, central pile.
    Centre,
    /// Front row first, then left, as far as they slide: a pile that fills
    /// a crowded plate's corners.
    Corner,
}

/// How a spot ranks, lowest first: the plate the pile needs, its area, and
/// how far the spot is from where the pass pulls.
type Score = (i64, i64, i64);

/// The search's working state for one pass.
#[derive(Debug)]
struct Pass<'p, 'a> {
    problem: &'p Problem<'a>,
    pull: Pull,
    metric: Metric,
    coarse: &'p [[i32; 2]],
    fine: &'p [[i32; 2]],
    rows: &'p [[i32; 2]],
    plate: Occupancy,
    /// The pile's bounds so far, `[lo, hi)` (cells).
    pile: Option<([i32; 2], [i32; 2])>,
}

impl Pass<'_, '_> {
    /// Where `shape` goes in this pass.
    fn spot(&self, shape: &Shape) -> Option<[i32; 2]> {
        self.spot_from(shape, &mut 0)
    }

    /// [`Self::spot`], looking from the `cursor`-th spot of the pass's
    /// order on, and leaving `cursor` at the first spot that fits. The plate
    /// only fills up within a pass, so a spot `shape` did not fit before
    /// stays full: the next copy of the same shape looks on from there.
    fn spot_from(&self, shape: &Shape, cursor: &mut usize) -> Option<[i32; 2]> {
        match self.pull {
            Pull::Centre => self.nearest(shape, cursor),
            Pull::Corner => self.cornered(shape, cursor),
        }
    }

    /// The first spot along the rows from `cursor` where `shape` fits, slid
    /// down and left a cell at a time while it fits.
    fn cornered(&self, shape: &Shape, cursor: &mut usize) -> Option<[i32; 2]> {
        let found = self.rows[(*cursor).min(self.rows.len())..]
            .iter()
            .map(|p| [p[0] - shape.lo[0], p[1] - shape.lo[1]])
            .position(|at| self.plate.fits(shape, at));
        let Some(found) = found else {
            *cursor = self.rows.len();
            return None;
        };
        *cursor += found;
        let p = self.rows[*cursor];
        let mut best = [p[0] - shape.lo[0], p[1] - shape.lo[1]];
        loop {
            let moved = [[0, -1], [-1, 0]]
                .into_iter()
                .map(|s| [best[0] + s[0], best[1] + s[1]])
                .find(|&at| self.plate.fits(shape, at));
            match moved {
                Some(at) => best = at,
                None => break,
            }
        }
        Some(best)
    }

    /// The spot nearest the target where `shape` fits, from the `cursor`-th
    /// coarse spot on, refined to the cell.
    fn nearest(&self, shape: &Shape, cursor: &mut usize) -> Option<[i32; 2]> {
        let anchor = shape.anchor();
        let goal = [
            self.problem.target[0] - anchor[0],
            self.problem.target[1] - anchor[1],
        ];
        let found = self.coarse[(*cursor).min(self.coarse.len())..]
            .iter()
            .map(|d| [goal[0] + d[0], goal[1] + d[1]])
            .position(|at| self.plate.fits(shape, at));
        let Some(found) = found else {
            *cursor = self.coarse.len();
            return None;
        };
        *cursor += found;
        let d = self.coarse[*cursor];
        let first = [goal[0] + d[0], goal[1] + d[1]];
        // Between the coarse spots: the nearest that fits around the one
        // found, then a cell at a time towards the goal while it fits.
        let mut best = first;
        if let Some(at) = self
            .fine
            .iter()
            .map(|d| [first[0] + d[0], first[1] + d[1]])
            .filter(|&at| self.metric.dist2(at, goal) < self.metric.dist2(first, goal))
            .find(|&at| self.plate.fits(shape, at))
        {
            best = at;
        }
        loop {
            let step = [(goal[0] - best[0]).signum(), (goal[1] - best[1]).signum()];
            let moved = [step, [step[0], 0], [0, step[1]]]
                .into_iter()
                .filter(|s| *s != [0, 0])
                .map(|s| [best[0] + s[0], best[1] + s[1]])
                .find(|&at| self.plate.fits(shape, at));
            match moved {
                Some(at) => best = at,
                None => break,
            }
        }
        Some(best)
    }

    /// The pile's bounds with `shape` at `at`.
    fn pile_with(&self, shape: &Shape, at: [i32; 2]) -> ([i32; 2], [i32; 2]) {
        let lo = [at[0] + shape.lo[0], at[1] + shape.lo[1]];
        let hi = [at[0] + shape.hi[0], at[1] + shape.hi[1]];
        match self.pile {
            None => (lo, hi),
            Some((a, b)) => (
                [a[0].min(lo[0]), a[1].min(lo[1])],
                [b[0].max(hi[0]), b[1].max(hi[1])],
            ),
        }
    }

    /// How `shape` at `at` ranks: the plate the pile then needs, its area,
    /// and how far the spot is from where the pass pulls.
    fn score(&self, shape: &Shape, at: [i32; 2]) -> Score {
        let (lo, hi) = self.pile_with(shape, at);
        let extent = self.metric.extent(lo, hi);
        let area = i64::from(hi[0] - lo[0]) * i64::from(hi[1] - lo[1]);
        let near = match self.pull {
            Pull::Centre => {
                let anchor = shape.anchor();
                self.metric
                    .dist2([at[0] + anchor[0], at[1] + anchor[1]], self.problem.target)
            }
            // The lowest top, then the leftmost.
            Pull::Corner => {
                i64::from(at[1] + shape.hi[1]) * (1 << 20) + i64::from(at[0] + shape.lo[0])
            }
        };
        (extent, area, near)
    }

    /// Puts `shape` of `body` at `at`.
    fn stamp(&mut self, body: usize, angle: usize, at: [i32; 2]) {
        let shape = &self.problem.footprints[body].shapes[angle];
        self.plate.stamp(shape, at);
        self.pile = Some(self.pile_with(shape, at));
    }

    /// Places `body` at its best angle and spot; `None` when it fits nowhere.
    fn place(&mut self, body: usize) -> Option<(usize, [i32; 2])> {
        let fp = &self.problem.footprints[body];
        // (extent, area, nearness), angle, spot.
        let mut best: Option<(Score, usize, [i32; 2])> = None;
        for (angle, shape) in fp.shapes.iter().enumerate() {
            let Some(at) = self.spot(shape) else {
                continue;
            };
            let score = self.score(shape, at);
            if best.is_none_or(|(s, _, _)| score < s) {
                best = Some((score, angle, at));
            }
        }
        let (_, angle, at) = best?;
        self.stamp(body, angle, at);
        Some((angle, at))
    }

    /// Places one more copy of `body` at the best of `angles` and its spot,
    /// looking for each angle on from its cursor in `cursors`; `nearest`
    /// ranks the spot's nearness first. `None` when it fits nowhere.
    fn place_copy(
        &mut self,
        body: usize,
        angles: &[usize],
        cursors: &mut [usize],
        nearest: bool,
    ) -> Option<(usize, [i32; 2])> {
        let fp = &self.problem.footprints[body];
        let mut best: Option<(Score, usize, [i32; 2])> = None;
        for &angle in angles {
            let shape = &fp.shapes[angle];
            let Some(at) = self.spot_from(shape, &mut cursors[angle]) else {
                continue;
            };
            let (extent, area, near) = self.score(shape, at);
            let score = if nearest {
                (near, extent, area)
            } else {
                (extent, area, near)
            };
            if best.is_none_or(|(s, _, _)| score < s) {
                best = Some((score, angle, at));
            }
        }
        let (_, angle, at) = best?;
        self.stamp(body, angle, at);
        Some((angle, at))
    }
}

/// The spots a pass tries, in order, for a problem's plate.
#[derive(Debug)]
struct Spots {
    metric: Metric,
    coarse: Vec<[i32; 2]>,
    fine: Vec<[i32; 2]>,
    rows: Vec<[i32; 2]>,
}

impl Spots {
    fn of(problem: &Problem<'_>) -> Self {
        let step = ((1.0 / problem.cell).round() as i32).max(1);
        let reach = [
            problem.plate.width as i32 / 2 + step,
            problem.plate.height as i32 / 2 + step,
        ];
        let metric = Metric {
            size: [problem.plate.width as i64, problem.plate.height as i64],
        };
        Self {
            metric,
            coarse: spiral(reach, step, metric),
            fine: spiral([step, step], 1, metric),
            rows: rows(
                [problem.plate.width as i32, problem.plate.height as i32],
                step,
            ),
        }
    }

    fn pass<'p, 'a>(&'p self, problem: &'p Problem<'a>, pull: Pull) -> Pass<'p, 'a> {
        Pass {
            problem,
            pull,
            metric: self.metric,
            coarse: &self.coarse,
            fine: &self.fine,
            rows: &self.rows,
            plate: problem.plate.clone(),
            pile: None,
        }
    }
}

/// Which angles the copies of a fill pass may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turns {
    /// Any the footprint has.
    All,
    /// Only the way the body stands: copies in rows, as on a tray.
    Own,
    /// The way it stands, or half a turn from it: pairs that nest.
    Half,
}

/// A copy a fill placed: which body it copies, its angle, and where its
/// frame's corner sits (plate cells).
pub(crate) type Copied = (usize, usize, [i32; 2]);

/// Places copies of the bodies of `problem` on its plate, which holds
/// everything that stays, until one does not fit or there are `limit`:
/// a copy of each body in turn, so each body gets as many as the others,
/// give or take one. Several passes run side by side (pulled to the
/// middle or to the corner, ranked by the pile or by nearness, with every
/// angle, only the body's own, or its own and half a turn) and the one
/// that places most wins, then the smallest pile. `placed` follows the
/// count of the pass furthest along. `None` when cancelled.
pub(crate) fn fill(
    problem: &Problem<'_>,
    limit: Option<usize>,
    placed: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<Vec<Copied>> {
    let bodies = problem.footprints.len();
    if bodies == 0 || limit == Some(0) {
        return Some(Vec::new());
    }
    let spots = Spots::of(problem);
    let turns = problem
        .footprints
        .iter()
        .map(|fp| fp.shapes.len())
        .min()
        .unwrap_or(1);
    let mut variants: Vec<(Pull, bool, Turns)> = Vec::new();
    for pull in [Pull::Corner, Pull::Centre] {
        for nearest in [true, false] {
            for t in [Turns::All, Turns::Own, Turns::Half] {
                let useful = match t {
                    Turns::Own => true,
                    Turns::All => turns > 1,
                    Turns::Half => turns > 2 && turns % 2 == 0,
                };
                if useful {
                    variants.push((pull, nearest, t));
                }
            }
        }
    }
    let runs: Vec<Option<(Vec<Copied>, i64)>> = variants
        .par_iter()
        .enumerate()
        .map(|(v, &(pull, nearest, t))| {
            let mut run = spots.pass(problem, pull);
            let angles: Vec<Vec<usize>> = problem
                .footprints
                .iter()
                .map(|fp| {
                    let n = fp.shapes.len();
                    match t {
                        Turns::All => (0..n).collect(),
                        Turns::Own => vec![0],
                        Turns::Half => vec![0, n / 2],
                    }
                })
                .collect();
            let mut cursors: Vec<Vec<usize>> = problem
                .footprints
                .iter()
                .map(|fp| vec![0; fp.shapes.len()])
                .collect();
            let mut copies: Vec<Copied> = Vec::new();
            // Each pass starts with another body, so no body is always the
            // one that gets the extra copy.
            let first = v % bodies;
            'fill: loop {
                for k in (0..bodies).map(|j| (first + j) % bodies) {
                    if limit.is_some_and(|l| copies.len() >= l) {
                        break 'fill;
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    match run.place_copy(k, &angles[k], &mut cursors[k], nearest) {
                        Some((angle, at)) => copies.push((k, angle, at)),
                        None => break 'fill,
                    }
                    placed.fetch_max(copies.len(), Ordering::Relaxed);
                }
            }
            let extent = run.pile.map_or(0, |(lo, hi)| spots.metric.extent(lo, hi));
            Some((copies, extent))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // The most copies, then the smallest pile; ties to the earlier pass.
    runs.into_iter()
        .flatten()
        .enumerate()
        .min_by_key(|(i, (copies, extent))| (std::cmp::Reverse(copies.len()), *extent, *i))
        .map(|(_, (copies, _))| copies)
}

/// Runs the passes and keeps the best layout. `None` when cancelled.
pub(crate) fn search(problem: &Problem<'_>, cancel: &AtomicBool) -> Option<Layout> {
    let spots = Spots::of(problem);
    let metric = spots.metric;
    let sizes: Vec<i64> = problem
        .footprints
        .iter()
        .map(|fp| fp.shapes.first().map_or(0, Shape::area))
        .collect();
    let layouts: Vec<Option<(Pull, Layout)>> = (0..problem.passes.max(2))
        .into_par_iter()
        .map(|pass| {
            // The first pass of each pull always runs to the end; the others
            // only start in time.
            if pass > 1 && Instant::now() >= problem.deadline {
                return None;
            }
            let mut rng = Rng::new(u64::from(pass) + 1);
            let mut order: Vec<(f64, usize)> = sizes
                .iter()
                .enumerate()
                .map(|(i, &size)| {
                    let shake = if pass < 2 {
                        1.0
                    } else {
                        1.0 + SHAKE * (2.0 * rng.next() - 1.0)
                    };
                    (size as f64 * shake, i)
                })
                .collect();
            order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            // Passes take turns pulling to the middle and to the corner.
            let pull = if pass % 2 == 0 {
                Pull::Centre
            } else {
                Pull::Corner
            };
            let mut run = spots.pass(problem, pull);
            let mut placed = vec![None; sizes.len()];
            for (_, body) in order {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                placed[body] = run.place(body);
            }
            Some((pull, finish(problem, metric, placed, run.pile, pass)))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut layouts: Vec<(Pull, Layout)> = layouts.into_iter().flatten().collect();
    // A pile that cannot shift stays where it was made: it only comes from
    // the corner when the middle leaves bodies off.
    if !problem.balance {
        let most = |pull: Pull| {
            layouts
                .iter()
                .filter(|(p, _)| *p == pull)
                .map(|(_, l)| l.count)
                .max()
        };
        if most(Pull::Centre) >= most(Pull::Corner) {
            layouts.retain(|(p, _)| *p == Pull::Centre);
        }
    }
    pick(layouts.into_iter().map(|(_, l)| l).collect())
}

/// The winner: most bodies placed, then the pile that needs the least
/// plate, then, within [`BALANCE_SLACK`] of it, the best balance; ties go
/// to the earlier pass.
fn pick(layouts: Vec<Layout>) -> Option<Layout> {
    let count = layouts.iter().map(|l| l.count).max()?;
    let fullest: Vec<Layout> = layouts.into_iter().filter(|l| l.count == count).collect();
    let smallest = fullest.iter().map(|l| l.extent).min()?;
    let limit = smallest as f64 * (1.0 + BALANCE_SLACK);
    fullest
        .into_iter()
        .filter(|l| l.extent as f64 <= limit)
        .min_by(|a, b| {
            a.imbalance
                .total_cmp(&b.imbalance)
                .then(a.pass.cmp(&b.pass))
        })
}

/// A pass's layout, with its balance and the shift that improves it.
fn finish(
    problem: &Problem<'_>,
    metric: Metric,
    placed: Vec<Option<(usize, [i32; 2])>>,
    pile: Option<([i32; 2], [i32; 2])>,
    pass: u32,
) -> Layout {
    let cell = problem.cell;
    let count = placed.iter().flatten().count();
    let extent = pile.map_or(0, |(lo, hi)| metric.extent(lo, hi));
    // Each band's area and area-weighted centroid sum, world mm.
    let mut bands: Vec<(f64, [f64; 2])> = Vec::new();
    for (body, at) in placed.iter().enumerate() {
        let Some((angle, at)) = *at else {
            continue;
        };
        let fp = &problem.footprints[body];
        let (s, c) = fp.angles[angle].sin_cos();
        let reference = [
            problem.origin[0] + f64::from(at[0] + fp.radius) * cell,
            problem.origin[1] + f64::from(at[1] + fp.radius) * cell,
        ];
        for peel in fp.peel.iter() {
            if bands.len() <= peel.band {
                bands.resize(peel.band + 1, (0.0, [0.0, 0.0]));
            }
            let [x, y] = peel.centroid;
            let p = [reference[0] + c * x - s * y, reference[1] + s * x + c * y];
            let slot = &mut bands[peel.band];
            slot.0 += peel.area;
            slot.1[0] += p[0] * peel.area;
            slot.1[1] += p[1] * peel.area;
        }
    }
    let total: f64 = bands.iter().map(|b| b.0).sum();
    let width = problem.plate.width as f64 * cell;
    let height = problem.plate.height as f64 * cell;
    let middle = [
        problem.origin[0] + width * 0.5,
        problem.origin[1] + height * 0.5,
    ];
    let mut shift = [0.0, 0.0];
    if problem.balance
        && total > 0.0
        && let Some((lo, hi)) = pile
    {
        let mass = [
            bands.iter().map(|b| b.1[0]).sum::<f64>() / total,
            bands.iter().map(|b| b.1[1]).sum::<f64>() / total,
        ];
        // The pile's centroid onto the plate's middle, as far as it stays
        // on the plate.
        let room = [
            (
                -f64::from(lo[0]) * cell,
                (problem.plate.width as f64 - f64::from(hi[0])) * cell,
            ),
            (
                -f64::from(lo[1]) * cell,
                (problem.plate.height as f64 - f64::from(hi[1])) * cell,
            ),
        ];
        for k in 0..2 {
            let (a, b) = room[k];
            shift[k] = (middle[k] - mass[k]).clamp(a.min(0.0), b.max(0.0));
        }
    }
    let imbalance = if total > 0.0 {
        bands
            .iter()
            .filter(|b| b.0 > 0.0)
            .map(|b| {
                let dx = b.1[0] / b.0 + shift[0] - middle[0];
                let dy = b.1[1] / b.0 + shift[1] - middle[1];
                (dx * dx + dy * dy).sqrt() * b.0
            })
            .sum::<f64>()
            / total
    } else {
        0.0
    };
    Layout {
        placed,
        count,
        extent,
        imbalance,
        shift,
        pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spiral_goes_out_from_the_middle() {
        let metric = Metric { size: [2, 1] };
        let s = spiral([8, 4], 4, metric);
        assert_eq!(s[0], [0, 0]);
        assert!(
            s.windows(2)
                .all(|w| metric.dist2(w[0], [0, 0]) <= metric.dist2(w[1], [0, 0]))
        );
        // Along the plate's long side, a step counts half.
        assert_eq!(s[1], [-4, 0]);
        assert_eq!(s.len(), 7 * 5);
    }

    #[test]
    fn the_generator_is_seeded() {
        let (mut a, mut b) = (Rng::new(3), Rng::new(3));
        let x: Vec<f64> = (0..4).map(|_| a.next()).collect();
        let y: Vec<f64> = (0..4).map(|_| b.next()).collect();
        assert_eq!(x, y);
        assert!(x.iter().all(|v| (0.0..1.0).contains(v)));
    }
}
