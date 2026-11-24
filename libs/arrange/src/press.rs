/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Presses one more copy onto a full plate (Fill bed).
//!
//! The copy goes in where it overlaps least, and the copies that overlap
//! move, one at a time, to where they overlap least, until none does: a
//! guided local search, as in Umetani et al.'s overlap minimisation for
//! nesting and Gardeyn and Wauters' sparrow. How deep two copies overlap is
//! read off their no-fit map ([`crate::nest`]): a move inside it is as deep
//! as it is far from the map's edge, the least either must move to clear the
//! other. So weighing a spot is a lookup per neighbour, exact band by band.
//!
//! Pairs that keep overlapping weigh more each round, so the copies around
//! them make room. Every try runs from its own seed, so the same scene
//! gives the same plate; several run side by side, and the first in their
//! order that clears every overlap wins.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::nest::{Free, Kinds};
use crate::search::Rng;

/// How deep each move lies in a no-fit map: the distance (cells, a third
/// each) to the nearest move outside it, 0 outside.
pub(crate) struct Depth {
    lo: [i32; 2],
    size: [usize; 2],
    depth: Vec<u8>,
}

impl Depth {
    /// The depths of `map` (moves `lo` on, `size` across and up, set where
    /// they collide), by a 3-4 chamfer.
    fn of(lo: [i32; 2], size: [usize; 2], set: impl Fn(usize, usize) -> bool) -> Depth {
        let [w, h] = size;
        let big = u16::MAX / 2;
        let mut d = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                if set(x, y) {
                    d[y * w + x] = big;
                }
            }
        }
        let at = |d: &Vec<u16>, x: isize, y: isize| -> u16 {
            if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                0
            } else {
                d[y as usize * w + x as usize]
            }
        };
        for y in 0..h as isize {
            for x in 0..w as isize {
                let i = y as usize * w + x as usize;
                if d[i] == 0 {
                    continue;
                }
                let v = d[i]
                    .min(at(&d, x - 1, y) + 3)
                    .min(at(&d, x, y - 1) + 3)
                    .min(at(&d, x - 1, y - 1) + 4)
                    .min(at(&d, x + 1, y - 1) + 4);
                d[i] = v;
            }
        }
        for y in (0..h as isize).rev() {
            for x in (0..w as isize).rev() {
                let i = y as usize * w + x as usize;
                if d[i] == 0 {
                    continue;
                }
                let v = d[i]
                    .min(at(&d, x + 1, y) + 3)
                    .min(at(&d, x, y + 1) + 3)
                    .min(at(&d, x + 1, y + 1) + 4)
                    .min(at(&d, x - 1, y + 1) + 4);
                d[i] = v;
            }
        }
        Depth {
            lo,
            size,
            depth: d.into_iter().map(|v| v.min(255) as u8).collect(),
        }
    }

    fn at(&self, m: [i32; 2]) -> u16 {
        let (x, y) = (m[0] - self.lo[0], m[1] - self.lo[1]);
        if x < 0 || y < 0 || x as usize >= self.size[0] || y as usize >= self.size[1] {
            return 0;
        }
        u16::from(self.depth[y as usize * self.size[0] + x as usize])
    }
}

/// The depths of every no-fit map of `kinds`, and of where what stays
/// blocks each kind.
pub(crate) struct Depths {
    pairs: Vec<Depth>,
    held: Vec<Option<Depth>>,
}

/// What the depths of a fill may take, at most (bytes).
const DEPTHS_BUDGET: usize = 256 << 20;

impl Depths {
    /// Whether the depths of `kinds` fit their budget: a byte per move of
    /// every map.
    pub(crate) fn fit(kinds: &Kinds) -> bool {
        let n = kinds.len();
        let bytes: usize = (0..n * n)
            .map(|k| {
                let s = kinds.nofit(k / n, k % n).size();
                s[0] * s[1]
            })
            .sum();
        bytes <= DEPTHS_BUDGET
    }

    pub(crate) fn of(kinds: &Kinds) -> Depths {
        let n = kinds.len();
        let pairs = (0..n * n)
            .into_par_iter()
            .map(|k| {
                let map = kinds.nofit(k / n, k % n);
                Depth::of(map.lo(), map.size(), |x, y| {
                    map.row(y)[x / 64] >> (x % 64) & 1 == 1
                })
            })
            .collect();
        let held = (0..n)
            .into_par_iter()
            .map(|k| {
                let map = kinds.held(k)?;
                Some(Depth::of(map.lo(), map.size(), |x, y| {
                    map.row(y)[x / 64] >> (x % 64) & 1 == 1
                }))
            })
            .collect();
        Depths { pairs, held }
    }
}

/// A copy in a try: its body, kind and frame's corner.
#[derive(Clone, Copy, Debug)]
struct Item {
    body: usize,
    kind: usize,
    at: [i32; 2],
}

/// How many rounds a try gets: each moves every copy that overlaps.
const ROUNDS: usize = 200;

/// How many rounds the tries made while time is left get.
const ROUNDS_LONG: usize = 1000;

/// How many tries run side by side for each copy added.
const TRIES: u64 = 32;

/// Spots a copy samples over the whole plate, then near where it stands,
/// before it steps downhill.
const SAMPLES_FAR: usize = 60;
const SAMPLES_NEAR: usize = 120;

/// How far (cells) a near sample may land from where the copy stands.
const NEAR: f64 = 16.0;

/// Spots the copy put in first samples over the plate.
const SAMPLES_FIRST: usize = 4000;

struct Try<'a> {
    kinds: &'a Kinds,
    depths: &'a Depths,
    /// Per body, its kinds.
    of_body: &'a [Vec<usize>],
    items: Vec<Item>,
    weights: Vec<f64>,
    held_weights: Vec<f64>,
}

impl Try<'_> {
    fn n(&self) -> usize {
        self.items.len()
    }

    /// Whether kind `k` at `at` lies on the plate.
    fn on_plate(&self, k: usize, at: [i32; 2]) -> bool {
        let room = self.kinds.room(k);
        let p = [at[0] + self.kinds.lo(k)[0], at[1] + self.kinds.lo(k)[1]];
        p[0] >= 0 && p[1] >= 0 && p[0] < room[0] && p[1] < room[1]
    }

    /// The overlap of copy `i` with copy `j`, were `i` kind `k` at `at`.
    fn pair(&self, i: usize, k: usize, at: [i32; 2], j: usize) -> u16 {
        let o = self.items[j];
        let n = self.kinds.len();
        // Kind k moved by at - o.at from kind o.kind.
        let _ = i;
        self.depths.pairs[o.kind * n + k].at([at[0] - o.at[0], at[1] - o.at[1]])
    }

    fn held(&self, k: usize, at: [i32; 2]) -> u16 {
        self.depths.held[k].as_ref().map_or(0, |d| d.at(at))
    }

    /// The weighted overlap of copy `i` as kind `k` at `at`, up to `cap`.
    fn cost(&self, i: usize, k: usize, at: [i32; 2], cap: f64) -> f64 {
        let n = self.n();
        let mut c = self.held_weights[i] * f64::from(self.held(k, at));
        if c >= cap {
            return c;
        }
        for j in 0..n {
            if j == i {
                continue;
            }
            let d = self.pair(i, k, at, j);
            if d > 0 {
                c += self.weights[i * n + j] * f64::from(d);
                if c >= cap {
                    return c;
                }
            }
        }
        c
    }

    fn random_spot(&self, k: usize, rng: &mut Rng) -> [i32; 2] {
        let room = self.kinds.room(k);
        let lo = self.kinds.lo(k);
        let pick = |n: i32, rng: &mut Rng| ((rng.next() * f64::from(n)) as i32).min(n - 1);
        [pick(room[0], rng) - lo[0], pick(room[1], rng) - lo[1]]
    }

    /// Where copy `i` overlaps least: `far` samples over the plate, `near`
    /// around where it stands, then steps downhill.
    fn best_spot(
        &self,
        i: usize,
        rng: &mut Rng,
        far: usize,
        near: usize,
    ) -> (usize, [i32; 2], f64) {
        let here = self.items[i];
        let kinds = &self.of_body[here.body];
        let mut best = (
            here.kind,
            here.at,
            self.cost(i, here.kind, here.at, f64::INFINITY),
        );
        let consider = |k: usize, at: [i32; 2], best: &mut (usize, [i32; 2], f64)| {
            if self.on_plate(k, at) {
                let c = self.cost(i, k, at, best.2);
                if c < best.2 {
                    *best = (k, at, c);
                }
            }
        };
        let pick_kind = |rng: &mut Rng| {
            kinds[((rng.next() * kinds.len() as f64) as usize).min(kinds.len() - 1)]
        };
        for _ in 0..far {
            let k = pick_kind(rng);
            let at = self.random_spot(k, rng);
            consider(k, at, &mut best);
            if best.2 == 0.0 {
                return best;
            }
        }
        for _ in 0..near {
            let k = if rng.next() < 0.7 {
                here.kind
            } else {
                pick_kind(rng)
            };
            let mut step = || ((rng.next() - 0.5) * 2.0 * NEAR).round() as i32;
            let at = [here.at[0] + step(), here.at[1] + step()];
            consider(k, at, &mut best);
            if best.2 == 0.0 {
                return best;
            }
        }
        for step in [8, 4, 2, 1] {
            loop {
                if best.2 == 0.0 {
                    return best;
                }
                let before = best.2;
                let (k, at, _) = best;
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
                    consider(k, [at[0] + d[0], at[1] + d[1]], &mut best);
                }
                if best.2 >= before {
                    break;
                }
            }
        }
        best
    }

    /// Every pair's overlap and each copy's with what stays.
    fn overlaps(&self) -> (Vec<u16>, Vec<u16>) {
        let n = self.n();
        let mut pairs = vec![0u16; n * n];
        let mut held = vec![0u16; n];
        for i in 0..n {
            let it = self.items[i];
            held[i] = self.held(it.kind, it.at);
            for j in i + 1..n {
                let o = self.pair(i, it.kind, it.at, j);
                pairs[i * n + j] = o;
                pairs[j * n + i] = o;
            }
        }
        (pairs, held)
    }
}

/// One try: its seed, its place among the tries run side by side, how many
/// rounds it gets, and when it must give up at the latest.
#[derive(Clone, Copy, Debug)]
struct Attempt {
    seed: u64,
    index: u64,
    rounds: usize,
    until: Option<Instant>,
}

/// One try at adding a copy of body `extra` to `placed`: where every copy
/// then goes, or `None`.
#[allow(clippy::too_many_arguments)]
fn separate(
    kinds: &Kinds,
    depths: &Depths,
    of_body: &[Vec<usize>],
    placed: &[Item],
    extra: usize,
    attempt: Attempt,
    found: &AtomicU64,
    cancel: &AtomicBool,
) -> Option<Vec<Item>> {
    let (seed, try_index) = (attempt.seed, attempt.index);
    let mut rng = Rng::new(seed);
    let mut items = placed.to_vec();
    let ks = &of_body[extra];
    let k = ks[((rng.next() * ks.len() as f64) as usize).min(ks.len() - 1)];
    let n = items.len() + 1;
    let mut state = Try {
        kinds,
        depths,
        of_body,
        items: Vec::new(),
        weights: vec![1.0; n * n],
        held_weights: vec![1.0; n],
    };
    let first = state.random_spot(k, &mut rng);
    items.push(Item {
        body: extra,
        kind: k,
        at: first,
    });
    state.items = items;
    let (k, at, _) = state.best_spot(n - 1, &mut rng, SAMPLES_FIRST, 0);
    state.items[n - 1].kind = k;
    state.items[n - 1].at = at;
    let mut best_overlap = u64::MAX;
    let mut best_items = state.items.clone();
    let mut strikes = 0;
    for _ in 0..attempt.rounds {
        if cancel.load(Ordering::Relaxed)
            || found.load(Ordering::Relaxed) < try_index
            || attempt.until.is_some_and(|u| Instant::now() >= u)
        {
            return None;
        }
        let (pairs, held) = state.overlaps();
        let overlap = pairs.iter().map(|&o| u64::from(o)).sum::<u64>() / 2
            + held.iter().map(|&o| u64::from(o)).sum::<u64>();
        if overlap == 0 {
            found.fetch_min(try_index, Ordering::Relaxed);
            return Some(state.items);
        }
        if overlap < best_overlap {
            best_overlap = overlap;
            best_items.clone_from(&state.items);
            strikes = 0;
        } else {
            strikes += 1;
            if strikes > 40 {
                state.items.clone_from(&best_items);
                strikes = 0;
            }
        }
        let most = pairs.iter().chain(&held).copied().max().unwrap_or(1).max(1) as f64;
        let grow = |w: &mut f64, o: u16| {
            if o > 0 {
                *w *= 1.2 + 0.8 * f64::from(o) / most;
            } else {
                *w = (*w * 0.95).max(1.0);
            }
        };
        for (w, &o) in state.weights.iter_mut().zip(&pairs) {
            grow(w, o);
        }
        for (w, &o) in state.held_weights.iter_mut().zip(&held) {
            grow(w, o);
        }
        let mut over: Vec<usize> = (0..n)
            .filter(|&i| held[i] > 0 || (0..n).any(|j| pairs[i * n + j] > 0))
            .collect();
        for k in (1..over.len()).rev() {
            let j = ((rng.next() * (k + 1) as f64) as usize).min(k);
            over.swap(k, j);
        }
        for i in over {
            let (k, at, _) = state.best_spot(i, &mut rng, SAMPLES_FAR, SAMPLES_NEAR);
            state.items[i].kind = k;
            state.items[i].at = at;
        }
    }
    None
}

/// Adds to `placed` (body, kind, frame's corner) copies of the bodies of
/// `wanted`, in its order, while one fits: each is pressed in, the copies
/// placed moving to make room; one that does not fit ends it. A copy the
/// usual tries do not fit in is tried again, with longer tries, while
/// `until` is ahead. `room` says, before each, whether to try it. Every
/// result is checked on the maps of `start`. `None` when cancelled.
#[allow(clippy::too_many_arguments)]
pub(crate) fn press_in(
    kinds: &Kinds,
    depths: &Depths,
    start: &Free,
    placed: Vec<(usize, usize, [i32; 2])>,
    wanted: impl IntoIterator<Item = usize>,
    mut room: impl FnMut(usize) -> bool,
    until: Option<Instant>,
    cancel: &AtomicBool,
) -> Option<Vec<(usize, usize, [i32; 2])>> {
    let count = kinds.of.iter().map(|&(b, _)| b + 1).max().unwrap_or(0);
    let mut of_body = vec![Vec::new(); count];
    for (k, &(b, _)) in kinds.of.iter().enumerate() {
        of_body[b].push(k);
    }
    let kind_of = |b: usize, a: usize| of_body[b][a];
    let mut items: Vec<Item> = placed
        .iter()
        .map(|&(b, a, at)| Item {
            body: b,
            kind: kind_of(b, a),
            at,
        })
        .collect();
    let tries = TRIES;
    for (round, extra) in wanted.into_iter().enumerate() {
        if !room(items.len()) {
            break;
        }
        let base = items.len() as u64 * 7919 + round as u64;
        let mut batch = 0u64;
        let next = loop {
            let found = AtomicU64::new(u64::MAX);
            let results: Vec<Option<Vec<Item>>> = (0..tries)
                .into_par_iter()
                .map(|t| {
                    let attempt = Attempt {
                        seed: base * 131 + batch * 1_000_003 + t,
                        index: t,
                        rounds: if batch == 0 { ROUNDS } else { ROUNDS_LONG },
                        until: (batch > 0).then_some(until).flatten(),
                    };
                    separate(
                        kinds, depths, &of_body, &items, extra, attempt, &found, cancel,
                    )
                })
                .collect();
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(next) = results.into_iter().flatten().next() {
                break Some(next);
            }
            batch += 1;
            if until.is_none_or(|u| Instant::now() >= u) {
                break None;
            }
        };
        let Some(next) = next else {
            break;
        };
        // What the try says clears, checked as a fill places it.
        let mut free = start.clone();
        for it in &next {
            let p = [
                it.at[0] + kinds.lo(it.kind)[0],
                it.at[1] + kinds.lo(it.kind)[1],
            ];
            if !free.fits(kinds, it.kind, p) {
                return Some(back(kinds, &items));
            }
            free.place(kinds, it.kind, p);
        }
        items = next;
    }
    Some(back(kinds, &items))
}

fn back(kinds: &Kinds, items: &[Item]) -> Vec<(usize, usize, [i32; 2])> {
    items
        .iter()
        .map(|it| (it.body, kinds.of[it.kind].1, it.at))
        .collect()
}
