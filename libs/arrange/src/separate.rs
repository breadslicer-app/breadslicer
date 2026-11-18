/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Squeezes one more body onto a packed plate (#555).
//!
//! The passes place bodies one after the other, each where it fits then:
//! the plate fills with holes no body fits in, though moving a few bodies
//! a little would join them. This puts the next body in anyway, wherever it
//! overlaps least, and then moves the bodies that overlap, one at a time, to
//! where they overlap least, until none does. Overlap is the cells two
//! bodies share in a band. A pair that keeps overlapping weighs more each
//! round, so the bodies around it make room for it: a guided local search,
//! as in Gardeyn and Wauters' separator for strip packing.
//!
//! Every try runs a fixed number of rounds from its own seed, so the same
//! scene gives the same plate; several tries run side by side, and the
//! first, in their order, that ends with no overlap wins.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::footprint::{BandMask, Footprint, Shape};
use crate::grid::BitGrid;
use crate::plate::{Occupancy, shared};
use crate::search::Rng;

/// A body's angle and where its frame's corner sits (plate cells).
pub(crate) type Spot = (usize, [i32; 2]);

/// How many rounds a try gets: each moves every body that overlaps.
const ROUNDS: usize = 30;

/// How many slices of height the search weighs the bodies in: each
/// takes the bands in it as one, covering as much, so a body is weighed in
/// a few masks however tall it stands.
pub(crate) const GROUPS: usize = 4;

/// How many tries run side by side for each body added.
const TRIES: u64 = 16;

/// Spots a body that overlaps samples over the whole plate, then near
/// where it stands, before it steps downhill.
const SAMPLES_FAR: usize = 50;
const SAMPLES_NEAR: usize = 100;

/// How far (cells) a near sample may land from where the body stands.
const NEAR_CELLS: f64 = 12.0;

/// Spots the body put in first samples over the plate.
const SAMPLES_FIRST: usize = 3000;

/// A body in a try: its footprint, and per angle which of its masks is in
/// each band.
struct Item<'a> {
    fp: &'a Footprint,
    bands: Vec<Vec<i16>>,
}

impl<'a> Item<'a> {
    fn new(fp: &'a Footprint) -> Self {
        let bands = fp
            .shapes
            .iter()
            .map(|shape| {
                let top = shape.bands.iter().map(|m| m.band + 1).max().unwrap_or(0);
                let mut of = vec![-1i16; top];
                for (k, m) in shape.bands.iter().enumerate() {
                    of[m.band] = k as i16;
                }
                of
            })
            .collect();
        Self { fp, bands }
    }

    fn shape(&self, spot: Spot) -> &Shape {
        &self.fp.shapes[spot.0]
    }
}

/// `shape` with every `group` bands in one, which covers what any of them
/// covers: where the grouped shapes keep clear, the shapes do.
pub(crate) fn grouped(shape: &Shape, group: usize) -> Shape {
    let group = group.max(1);
    let mut joined: Vec<(usize, Vec<&BandMask>)> = Vec::new();
    for mask in &shape.bands {
        let g = mask.band / group;
        match joined.iter_mut().find(|(k, _)| *k == g) {
            Some((_, masks)) => masks.push(mask),
            None => joined.push((g, vec![mask])),
        }
    }
    let bands = joined
        .into_iter()
        .map(|(band, masks)| {
            let x0 = masks.iter().map(|m| m.x0).min().unwrap_or(0);
            let y0 = masks.iter().map(|m| m.y0).min().unwrap_or(0);
            let x1 = masks
                .iter()
                .map(|m| m.x0 + m.grid.width as i32)
                .max()
                .unwrap_or(0);
            let y1 = masks
                .iter()
                .map(|m| m.y0 + m.grid.height as i32)
                .max()
                .unwrap_or(0);
            let mut grid = BitGrid::new((x1 - x0) as usize, (y1 - y0) as usize);
            for m in &masks {
                let (q, s) = ((m.x0 - x0) as usize / 64, (m.x0 - x0) as u32 % 64);
                for &r in &m.rows {
                    let row = grid.row_mut((m.y0 - y0) as usize + r as usize);
                    for (k, &w) in m.grid.row(r as usize).iter().enumerate() {
                        if let Some(d) = row.get_mut(q + k) {
                            *d |= w << s;
                        }
                        if s > 0
                            && let Some(d) = row.get_mut(q + k + 1)
                        {
                            *d |= w >> (64 - s);
                        }
                    }
                }
            }
            let filled: Vec<u32> = (0..grid.height as u32)
                .filter(|&y| grid.row(y as usize).iter().any(|&w| w != 0))
                .collect();
            // Middle rows first, then outwards, as a band's are.
            let mid = filled.len() / 2;
            let rows = (0..filled.len())
                .map(|k| {
                    filled[if k % 2 == 0 {
                        mid + k / 2
                    } else {
                        mid - 1 - k / 2
                    }]
                })
                .collect();
            BandMask {
                band,
                x0,
                y0,
                grid,
                rows,
            }
        })
        .collect();
    Shape {
        bands,
        lo: shape.lo,
        hi: shape.hi,
    }
}

/// `fp` with its shapes [`grouped`].
fn grouped_footprint(fp: &Footprint, group: usize) -> Footprint {
    Footprint {
        shapes: fp.shapes.iter().map(|s| grouped(s, group)).collect(),
        ..fp.clone()
    }
}

/// The cells `a` at `sa` and `b` at `sb` share, counted up to `cap`.
fn pair(a: &Item<'_>, sa: Spot, b: &Item<'_>, sb: Spot, cap: u32) -> u32 {
    let (ta, tb) = (a.shape(sa), b.shape(sb));
    let lo = |t: &Shape, s: Spot| [s.1[0] + t.lo[0], s.1[1] + t.lo[1]];
    let hi = |t: &Shape, s: Spot| [s.1[0] + t.hi[0], s.1[1] + t.hi[1]];
    let (alo, ahi, blo, bhi) = (lo(ta, sa), hi(ta, sa), lo(tb, sb), hi(tb, sb));
    if alo[0] >= bhi[0] || blo[0] >= ahi[0] || alo[1] >= bhi[1] || blo[1] >= ahi[1] {
        return 0;
    }
    let of = &b.bands[sb.0];
    let mut n = 0;
    for ma in &ta.bands {
        let Some(&k) = of.get(ma.band) else {
            continue;
        };
        if k < 0 {
            continue;
        }
        let mb = &tb.bands[k as usize];
        // `ma` as it lies over `mb`'s grid.
        let at = [sa.1[0] - sb.1[0] - mb.x0, sa.1[1] - sb.1[1] - mb.y0];
        n += shared(&mb.grid, ma, at, cap - n);
        if n >= cap {
            return n;
        }
    }
    n
}

/// A try: the bodies, where they stand, and how much each pair's overlap
/// weighs.
struct State<'a> {
    items: Vec<Item<'a>>,
    spots: Vec<Spot>,
    /// Pair weights, row by row.
    weights: Vec<f64>,
    /// Each body's weight against what stays put.
    fixed_weights: Vec<f64>,
    fixed: &'a Occupancy,
}

impl State<'_> {
    /// The weighted overlap of body `i` at `spot` with the others and what
    /// stays put, counted up to `cap`.
    fn cost(&self, i: usize, spot: Spot, cap: f64) -> f64 {
        let n = self.items.len();
        let room = |c: f64, w: f64| ((cap - c) / w).ceil().clamp(1.0, f64::from(u32::MAX)) as u32;
        let mut c = 0.0;
        let w = self.fixed_weights[i];
        c += w * f64::from(
            self.fixed
                .overlap(self.items[i].shape(spot), spot.1, room(c, w)),
        );
        if c >= cap {
            return c;
        }
        for j in (0..n).filter(|&j| j != i) {
            let w = self.weights[i * n + j];
            c += w * f64::from(pair(
                &self.items[i],
                spot,
                &self.items[j],
                self.spots[j],
                room(c, w),
            ));
            if c >= cap {
                return c;
            }
        }
        c
    }

    /// Whether body `i` at `spot` lies on the plate.
    fn on_plate(&self, i: usize, spot: Spot) -> bool {
        self.fixed.holds(self.items[i].shape(spot), spot.1)
    }

    /// A spot on the plate for body `i` at angle `a`, at random.
    fn random_spot(&self, i: usize, a: usize, rng: &mut Rng) -> Option<Spot> {
        let shape = &self.items[i].fp.shapes[a];
        let lo = [-shape.lo[0], -shape.lo[1]];
        let hi = [
            self.fixed.width as i32 - shape.hi[0],
            self.fixed.height as i32 - shape.hi[1],
        ];
        if hi[0] < lo[0] || hi[1] < lo[1] {
            return None;
        }
        let pick = |k: usize, rng: &mut Rng| {
            lo[k] + ((rng.next() * f64::from(hi[k] - lo[k] + 1)) as i32).min(hi[k] - lo[k])
        };
        Some((a, [pick(0, rng), pick(1, rng)]))
    }

    /// Where body `i` overlaps least: `far` samples over the plate, `near`
    /// around where it stands, then steps downhill.
    fn best_spot(&self, i: usize, rng: &mut Rng, far: usize, near: usize) -> (Spot, f64) {
        let here = self.spots[i];
        let angles = self.items[i].fp.shapes.len();
        let mut best = (here, self.cost(i, here, f64::INFINITY));
        let consider = |spot: Spot, best: &mut (Spot, f64)| {
            if self.on_plate(i, spot) {
                let c = self.cost(i, spot, best.1);
                if c < best.1 {
                    *best = (spot, c);
                }
            }
        };
        let angle = |rng: &mut Rng| ((rng.next() * angles as f64) as usize).min(angles - 1);
        for _ in 0..far {
            let a = angle(rng);
            if let Some(spot) = self.random_spot(i, a, rng) {
                consider(spot, &mut best);
            }
            if best.1 == 0.0 {
                return best;
            }
        }
        for _ in 0..near {
            let a = if rng.next() < 0.7 { here.0 } else { angle(rng) };
            let mut step = || ((rng.next() - 0.5) * 2.0 * NEAR_CELLS).round() as i32;
            let spot = (a, [here.1[0] + step(), here.1[1] + step()]);
            consider(spot, &mut best);
            if best.1 == 0.0 {
                return best;
            }
        }
        for step in [8, 4, 2, 1] {
            loop {
                if best.1 == 0.0 {
                    return best;
                }
                let before = best.1;
                let (a, at) = best.0;
                for d in [
                    [step, 0],
                    [-step, 0],
                    [0, step],
                    [0, -step],
                    [step, step],
                    [-step, -step],
                    [step, -step],
                    [-step, step],
                ] {
                    consider((a, [at[0] + d[0], at[1] + d[1]]), &mut best);
                }
                if step == 1 && angles > 1 {
                    for da in [1, angles - 1] {
                        consider(((a + da) % angles, at), &mut best);
                    }
                }
                if best.1 >= before {
                    break;
                }
            }
        }
        best
    }

    /// Every pair's overlap, row by row, and each body's with what stays.
    fn overlaps(&self) -> (Vec<u32>, Vec<u32>) {
        let n = self.items.len();
        let mut pairs = vec![0u32; n * n];
        let mut fixed = vec![0u32; n];
        for i in 0..n {
            let spot = self.spots[i];
            fixed[i] = self
                .fixed
                .overlap(self.items[i].shape(spot), spot.1, u32::MAX);
            for j in i + 1..n {
                let o = pair(
                    &self.items[i],
                    spot,
                    &self.items[j],
                    self.spots[j],
                    u32::MAX,
                );
                pairs[i * n + j] = o;
                pairs[j * n + i] = o;
            }
        }
        (pairs, fixed)
    }
}

/// One try at adding body `extra` to `placed` (footprint index and spot)
/// on `fixed`, from `seed`: where every body then goes, or `None`.
fn separate(
    footprints: &[Footprint],
    fixed: &Occupancy,
    placed: &[(usize, Spot)],
    extra: usize,
    (seed, try_index): (u64, u64),
    found: &AtomicU64,
    cancel: &AtomicBool,
) -> Option<Vec<Spot>> {
    let mut rng = Rng::new(seed);
    let mut items: Vec<Item<'_>> = placed
        .iter()
        .map(|&(b, _)| Item::new(&footprints[b]))
        .collect();
    items.push(Item::new(&footprints[extra]));
    let n = items.len();
    let mut state = State {
        items,
        spots: placed.iter().map(|&(_, s)| s).collect(),
        weights: vec![1.0; n * n],
        fixed_weights: vec![1.0; n],
        fixed,
    };
    let angles = footprints[extra].shapes.len();
    let a = ((rng.next() * angles as f64) as usize).min(angles - 1);
    let first = state.random_spot(n - 1, a, &mut rng)?;
    state.spots.push(first);
    state.spots[n - 1] = state.best_spot(n - 1, &mut rng, SAMPLES_FIRST, 0).0;
    let mut best_overlap = u64::MAX;
    let mut best_spots = state.spots.clone();
    let mut strikes = 0;
    for _ in 0..ROUNDS {
        // A try before this one already found room: this one's answer
        // would not be taken.
        if cancel.load(Ordering::Relaxed) || found.load(Ordering::Relaxed) < try_index {
            return None;
        }
        let (pairs, alone) = state.overlaps();
        let overlap = pairs.iter().map(|&o| u64::from(o)).sum::<u64>() / 2
            + alone.iter().map(|&o| u64::from(o)).sum::<u64>();
        if overlap == 0 {
            found.fetch_min(try_index, Ordering::Relaxed);
            return Some(state.spots);
        }
        if overlap < best_overlap {
            best_overlap = overlap;
            best_spots.clone_from(&state.spots);
            strikes = 0;
        } else {
            strikes += 1;
            // Stuck: back to the least overlap seen, the weights kept.
            if strikes > 40 {
                state.spots.clone_from(&best_spots);
                strikes = 0;
            }
        }
        // Pairs that overlap weigh more, by how much; the others relax.
        let most = pairs
            .iter()
            .chain(&alone)
            .copied()
            .max()
            .unwrap_or(1)
            .max(1) as f64;
        let grow = |w: &mut f64, o: u32| {
            if o > 0 {
                *w *= 1.2 + 0.8 * f64::from(o) / most;
            } else {
                *w = (*w * 0.95).max(1.0);
            }
        };
        for (w, &o) in state.weights.iter_mut().zip(&pairs) {
            grow(w, o);
        }
        for (w, &o) in state.fixed_weights.iter_mut().zip(&alone) {
            grow(w, o);
        }
        // The bodies that overlap move, in a shuffled order.
        let mut over: Vec<usize> = (0..n)
            .filter(|&i| alone[i] > 0 || (0..n).any(|j| pairs[i * n + j] > 0))
            .collect();
        for k in (1..over.len()).rev() {
            let j = ((rng.next() * (k + 1) as f64) as usize).min(k);
            over.swap(k, j);
        }
        for i in over {
            state.spots[i] = state.best_spot(i, &mut rng, SAMPLES_FAR, SAMPLES_NEAR).0;
        }
    }
    None
}

/// Adds to `placed` (footprint index and spot, on `fixed`) the bodies of
/// `wanted`, in its order, while one fits: each next one is squeezed in,
/// the bodies placed moving to make room; one that does not fit ends it.
/// `room` says, before each, whether to try it. Returns where every body
/// goes then, those added last. `None` when cancelled.
pub(crate) fn squeeze_in(
    footprints: &[Footprint],
    fixed: &Occupancy,
    placed: Vec<(usize, Spot)>,
    wanted: impl IntoIterator<Item = usize>,
    mut room: impl FnMut(usize) -> bool,
    cancel: &AtomicBool,
) -> Option<Vec<(usize, Spot)>> {
    let mut placed = placed;
    let bands = footprints
        .iter()
        .flat_map(|fp| fp.shapes.iter())
        .flat_map(|s| s.bands.iter().map(|m| m.band + 1))
        .max()
        .unwrap_or(1);
    let group = bands.div_ceil(GROUPS);
    let coarse: Vec<Footprint> = footprints
        .par_iter()
        .map(|fp| grouped_footprint(fp, group))
        .collect();
    let coarse_fixed = fixed.grouped(group);
    for (k, extra) in wanted.into_iter().enumerate() {
        if !room(placed.len()) {
            break;
        }
        let round = placed.len() as u64 * 7919 + k as u64;
        let found = AtomicU64::new(u64::MAX);
        let tries: Vec<Option<Vec<Spot>>> = (0..TRIES)
            .into_par_iter()
            .map(|t| {
                separate(
                    &coarse,
                    &coarse_fixed,
                    &placed,
                    extra,
                    (round * 131 + t, t),
                    &found,
                    cancel,
                )
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Some(spots) = tries.into_iter().flatten().next() else {
            break;
        };
        let bodies: Vec<usize> = placed.iter().map(|&(b, _)| b).chain([extra]).collect();
        // What the try says clears, checked as a pass places it.
        let mut plate = fixed.clone();
        for (&b, &spot) in bodies.iter().zip(&spots) {
            let shape = &footprints[b].shapes[spot.0];
            if !plate.fits(shape, spot.1) {
                return Some(placed);
            }
            plate.stamp(shape, spot.1);
        }
        placed = bodies.into_iter().zip(spots).collect();
    }
    Some(placed)
}
