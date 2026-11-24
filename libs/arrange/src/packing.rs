/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! How Fill bed packs its copies, on the maps of [`crate::nest`].
//!
//! A quick fill places a copy of each body in turn at the first spot from a
//! corner of the plate, at the angle whose spot is nearest that corner,
//! until one no longer fits. Many quick fills run, from each corner, with
//! every angle allowed, one angle for all, or one and half a turn from it.
//! The best few then go on as a pilot: before each copy, every angle it
//! may take is tried by filling the rest of the plate quickly from there,
//! and the one that ends with most copies is kept. The pilot never ends
//! with fewer copies than the quick fill it starts from.
//!
//! A single body's copies also start from the patterns that lay most of
//! them ([`crate::pattern`]), the room each leaves filled quickly. The fill
//! that holds most wins; on a tie, the first tried.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::nest::{Corner, Free, Kinds};

/// Which angles the copies may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turns {
    All,
    One(usize),
    /// This one or half a turn from it.
    Half(usize),
}

/// How a quick fill places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Policy {
    corner: Corner,
    /// Ranks spots by the far side of the copy from the corner (rows of
    /// even height) instead of its near side.
    by_far: bool,
    turns: Turns,
}

/// What every fill of one plate shares.
struct Run<'a> {
    kinds: &'a Kinds,
    /// Per body, its kinds by angle.
    bodies: Vec<Vec<usize>>,
    /// The most copies a fill places.
    limit: usize,
    /// The plate's size (cells).
    plate: [i32; 2],
}

impl Run<'_> {
    /// The kinds body `b` may take under `turns`.
    fn allowed(&self, b: usize, turns: Turns) -> Vec<usize> {
        let ks = &self.bodies[b];
        let n = ks.len();
        if n == 0 {
            return Vec::new();
        }
        match turns {
            Turns::All => ks.clone(),
            Turns::One(a) => vec![ks[a % n]],
            Turns::Half(a) => {
                let mut v = vec![ks[a % n]];
                if n.is_multiple_of(2) && n > 1 {
                    v.push(ks[(a + n / 2) % n]);
                }
                v
            }
        }
    }

    /// How near the corner a copy of kind `k` at map spot `p` sits: its
    /// row, then along the row. Lower is nearer.
    fn rank(&self, k: usize, p: [i32; 2], policy: Policy) -> (i32, i32) {
        let size = self.kinds.size(k);
        let near_y = if policy.corner.top {
            self.plate[1] - (p[1] + size[1])
        } else {
            p[1]
        };
        let y = if policy.by_far {
            near_y + size[1]
        } else {
            near_y
        };
        let x = if policy.corner.right {
            self.plate[0] - (p[0] + size[0])
        } else {
            p[0]
        };
        (y, x)
    }

    /// A finished fill's worth: most copies, then the copies nearest the
    /// corner (the room left in one piece).
    fn worth(&self, free: &Free, policy: Policy) -> Worth {
        let spread: i64 = free
            .placed
            .iter()
            .map(|&(k, p)| {
                let (y, x) = self.rank(k, p, policy);
                i64::from(y) * 4 + i64::from(x)
            })
            .sum();
        (free.placed.len(), -spread)
    }

    /// Places one more copy of body `b` as `policy` does; whether it fit.
    fn step(&self, free: &mut Free, b: usize, policy: Policy) -> bool {
        let mut best: Option<((i32, i32), usize, [i32; 2])> = None;
        for k in self.allowed(b, policy.turns) {
            if let Some(p) = free.first(self.kinds, k, policy.corner) {
                let r = self.rank(k, p, policy);
                if best.is_none_or(|(br, _, _)| r < br) {
                    best = Some((r, k, p));
                }
            }
        }
        match best {
            Some((_, k, p)) => {
                free.place(self.kinds, k, p);
                true
            }
            None => false,
        }
    }

    /// Fills `free` quickly from body `next` on, a copy of each body in
    /// turn, until one does not fit or the limit is reached.
    fn quick(&self, free: &mut Free, mut next: usize, policy: Policy) {
        let n = self.bodies.len();
        while free.placed.len() < self.limit {
            if !self.step(free, next, policy) {
                break;
            }
            next = (next + 1) % n;
        }
    }

    /// A pilot from `start`: before each copy, each angle it may take is
    /// tried by a quick fill to the end; the best is kept. `None` when
    /// cancelled.
    fn pilot(
        &self,
        start: &Free,
        first: usize,
        policy: Policy,
        cancel: &AtomicBool,
    ) -> Option<Free> {
        let n = self.bodies.len();
        let mut free = start.clone();
        let mut next = first;
        while free.placed.len() < self.limit {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let tried: Vec<Option<(Worth, Free)>> = self
                .allowed(next, policy.turns)
                .par_iter()
                .map(|&k| {
                    let mut child = free.clone();
                    let p = child.first(self.kinds, k, policy.corner)?;
                    child.place(self.kinds, k, p);
                    let mut rest = child.clone();
                    self.quick(&mut rest, (next + 1) % n, policy);
                    Some((self.worth(&rest, policy), child))
                })
                .collect();
            let Some((_, child)) = tried
                .into_iter()
                .flatten()
                .enumerate()
                .max_by(|(i, (a, _)), (j, (b, _))| a.cmp(b).then(j.cmp(i)))
                .map(|(_, t)| t)
            else {
                break;
            };
            free = child;
            next = (next + 1) % n;
        }
        Some(free)
    }
}

/// A finished fill's worth: most copies, then the copies nearest the
/// corner.
type Worth = (usize, i64);

/// How many of the patterns that lay most seed a fill.
const SEEDS: usize = 6;

/// How many quick fills go on as pilots.
const PILOTS: usize = 6;

/// The copies a fill on `start` places, a copy of each body in turn from
/// body `first`, up to `limit`, on a plate `plate` cells across and up:
/// (body, angle, frame's corner), in the order placed. `None` when
/// cancelled.
pub(crate) fn pack(
    kinds: &Kinds,
    start: &Free,
    first: usize,
    limit: usize,
    plate: [i32; 2],
    cancel: &AtomicBool,
) -> Option<Vec<(usize, usize, [i32; 2])>> {
    let count = kinds.of.iter().map(|&(b, _)| b + 1).max().unwrap_or(0);
    let mut bodies = vec![Vec::new(); count];
    for (k, &(b, _)) in kinds.of.iter().enumerate() {
        bodies[b].push(k);
    }
    let run = Run {
        kinds,
        bodies,
        limit,
        plate,
    };
    let turns = run.bodies.iter().map(Vec::len).min().unwrap_or(0);
    if turns == 0 {
        return Some(Vec::new());
    }
    let mut policies = Vec::new();
    for top in [false, true] {
        for right in [false, true] {
            for by_far in [false, true] {
                let corner = Corner { top, right };
                let mut add = |turns| {
                    policies.push(Policy {
                        corner,
                        by_far,
                        turns,
                    })
                };
                add(Turns::All);
                for a in 0..turns {
                    add(Turns::One(a));
                }
                if turns > 2 && turns.is_multiple_of(2) {
                    for a in 0..turns / 2 {
                        add(Turns::Half(a));
                    }
                }
            }
        }
    }
    let quick_fills: Vec<(Worth, Free)> = policies
        .par_iter()
        .map(|&policy| {
            let mut free = start.clone();
            run.quick(&mut free, first, policy);
            (run.worth(&free, policy), free)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // A single body's copies also start from the patterns that lay most,
    // the room they leave filled quickly.
    let seeded: Vec<Free> = if run.bodies.len() == 1 {
        let laid = crate::pattern::patterns(kinds, start, &run.bodies[0], cancel)?;
        laid.par_iter()
            .take(SEEDS)
            .filter_map(|spots| {
                let mut seed = start.clone();
                for &(k, at) in spots.iter().take(limit) {
                    let lo = kinds.lo(k);
                    let p = [at[0] + lo[0], at[1] + lo[1]];
                    if seed.fits(kinds, k, p) {
                        seed.place(kinds, k, p);
                    }
                }
                policies
                    .iter()
                    .map(|&policy| {
                        let mut free = seed.clone();
                        run.quick(&mut free, first, policy);
                        free
                    })
                    .max_by_key(|f| f.placed.len())
            })
            .collect()
    } else {
        Vec::new()
    };
    // The pilots start from the policies whose quick fills hold most.
    let mut order: Vec<usize> = (0..quick_fills.len()).collect();
    order.sort_by(|&a, &b| quick_fills[b].0.0.cmp(&quick_fills[a].0.0).then(a.cmp(&b)));
    order.truncate(PILOTS);
    let piloted: Vec<Option<Free>> = order
        .par_iter()
        .map(|&i| run.pilot(start, first, policies[i], cancel))
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let best = piloted
        .into_iter()
        .flatten()
        .chain(seeded)
        .chain(quick_fills.into_iter().map(|(_, f)| f))
        .enumerate()
        .max_by(|(i, a), (j, b)| a.placed.len().cmp(&b.placed.len()).then(j.cmp(i)))
        .map(|(_, f)| f)?;
    Some(
        best.placed
            .iter()
            .map(|&(k, p)| {
                let (b, a) = kinds.of[k];
                (b, a, kinds.at(k, p))
            })
            .collect(),
    )
}
