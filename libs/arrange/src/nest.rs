/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where copies still fit, kept as maps (Fill bed).
//!
//! A kind is a body at one of its angles. For every two kinds, a no-fit map
//! says at which moves of the second from the first they collide in some
//! band ([`Nofit`]); for each kind, another says where what stays on the
//! plate blocks it. A layout keeps, per kind, a bit per spot the kind's
//! frame may take on the plate: set where it no longer fits. Placing a copy
//! sets, in every kind's map, the spots its no-fit map covers there. So
//! the first free spot of a kind, in any order, is a scan for a clear bit,
//! and a whole fill takes well under a millisecond: thousands of fills can
//! be tried where one was.

use std::sync::Arc;

use crate::footprint::Shape;
use crate::grid::BitGrid;
use crate::pattern::{Nofit, Runs};
use crate::plate::Occupancy;

use rayon::prelude::*;

/// Every kind's shape, where its frame may go, and the no-fit maps.
pub(crate) struct Kinds {
    /// Per kind: its body (an index among the bodies copied) and angle.
    pub(crate) of: Vec<(usize, usize)>,
    /// Per kind: the corner of what it covers in its frame, and its size
    /// (cells).
    lo: Vec<[i32; 2]>,
    size: Vec<[i32; 2]>,
    /// The plate's size (cells).
    plate: [i32; 2],
    /// `nofit[a * n + b]`: where kind `b` collides with kind `a`.
    nofit: Arc<Vec<Nofit>>,
    /// Each kind's cells as runs, for the maps of another plate.
    runs: Arc<Vec<Runs>>,
    /// Where what stays on the plate blocks each kind.
    held: Vec<Option<Nofit>>,
}

impl Kinds {
    /// The kinds of `bodies`' shapes (one list of shapes, an angle each, per
    /// body), on `plate`.
    pub(crate) fn new(bodies: &[&[Shape]], plate: &Occupancy) -> Kinds {
        let mut of = Vec::new();
        let mut shapes: Vec<&Shape> = Vec::new();
        for (b, list) in bodies.iter().enumerate() {
            for (a, s) in list.iter().enumerate() {
                of.push((b, a));
                shapes.push(s);
            }
        }
        let n = shapes.len();
        let runs: Vec<Runs> = shapes.par_iter().map(|s| Runs::of(s)).collect();
        // A kind against another is the other against it, turned half a
        // turn: each pair is drawn once.
        let half: Vec<Nofit> = (0..n * (n + 1) / 2)
            .into_par_iter()
            .map(|k| {
                let (a, b) = pair_of(k);
                Nofit::of(&runs[a], &runs[b])
            })
            .collect();
        let nofit: Vec<Nofit> = (0..n * n)
            .into_par_iter()
            .map(|k| {
                let (a, b) = (k / n, k % n);
                if a <= b {
                    half[b * (b + 1) / 2 + a].clone()
                } else {
                    half[a * (a + 1) / 2 + b].reflected()
                }
            })
            .collect();
        let mut kinds = Kinds {
            of,
            lo: shapes.iter().map(|s| s.lo).collect(),
            size: shapes
                .iter()
                .map(|s| [s.hi[0] - s.lo[0], s.hi[1] - s.lo[1]])
                .collect(),
            plate: [plate.width as i32, plate.height as i32],
            nofit: Arc::new(nofit),
            runs: Arc::new(runs),
            held: Vec::new(),
        };
        kinds.held = kinds.held_on(plate);
        kinds
    }

    /// The same kinds on `plate`, which holds other bodies that stay.
    pub(crate) fn on(&self, plate: &Occupancy) -> Kinds {
        Kinds {
            of: self.of.clone(),
            lo: self.lo.clone(),
            size: self.size.clone(),
            plate: [plate.width as i32, plate.height as i32],
            nofit: self.nofit.clone(),
            runs: self.runs.clone(),
            held: self.held_on(plate),
        }
    }

    fn held_on(&self, plate: &Occupancy) -> Vec<Option<Nofit>> {
        let stays = Runs::of_plate(plate);
        self.runs
            .par_iter()
            .map(|r| (!stays.is_empty()).then(|| Nofit::of(&stays, r)))
            .collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.of.len()
    }

    /// Where kind `b` collides with kind `a`.
    pub(crate) fn nofit(&self, a: usize, b: usize) -> &Nofit {
        &self.nofit[a * self.len() + b]
    }

    /// Where what stays blocks kind `k`.
    pub(crate) fn held(&self, k: usize) -> Option<&Nofit> {
        self.held[k].as_ref()
    }

    /// The corner of what kind `k` covers in its frame.
    pub(crate) fn lo(&self, k: usize) -> [i32; 2] {
        self.lo[k]
    }

    /// How many spots kind `k`'s frame may take across and up.
    pub(crate) fn room(&self, k: usize) -> [i32; 2] {
        [
            self.plate[0] - self.size[k][0] + 1,
            self.plate[1] - self.size[k][1] + 1,
        ]
    }

    /// The frame's corner of kind `k` at map spot `p`.
    pub(crate) fn at(&self, k: usize, p: [i32; 2]) -> [i32; 2] {
        [p[0] - self.lo[k][0], p[1] - self.lo[k][1]]
    }

    /// Kind `k`'s size (cells).
    pub(crate) fn size(&self, k: usize) -> [i32; 2] {
        self.size[k]
    }
}

/// A layout in progress: per kind, the spots taken.
#[derive(Clone)]
pub(crate) struct Free {
    maps: Vec<BitGrid>,
    /// Per kind, the first row that may still have a clear spot.
    first_row: Vec<i32>,
    /// The copies placed: kind and map spot.
    pub(crate) placed: Vec<(usize, [i32; 2])>,
}

/// The pair `(a, b)`, `a <= b`, at index `k` of the pairs listed `b` by
/// `b`, each with every `a` up to it.
fn pair_of(k: usize) -> (usize, usize) {
    let mut b = ((((8 * k + 1) as f64).sqrt() - 1.0) / 2.0) as usize;
    while (b + 1) * (b + 2) / 2 <= k {
        b += 1;
    }
    while b * (b + 1) / 2 > k {
        b -= 1;
    }
    (k - b * (b + 1) / 2, b)
}

/// `dst` |= `src` moved `shift` bits up (down when negative); bits past
/// `dst` are dropped.
fn or_shifted(dst: &mut [u64], src: &[u64], shift: i32) {
    let n = dst.len() as isize;
    if shift >= 0 {
        let (q, s) = ((shift / 64) as isize, (shift % 64) as u32);
        for (i, &w) in src.iter().enumerate() {
            if w == 0 {
                continue;
            }
            let at = i as isize + q;
            if at >= n {
                break;
            }
            dst[at as usize] |= w << s;
            if s > 0 && at + 1 < n {
                dst[at as usize + 1] |= w >> (64 - s);
            }
        }
    } else {
        let t = -shift;
        let (q, s) = ((t / 64) as isize, (t % 64) as u32);
        for (i, &w) in src.iter().enumerate() {
            if w == 0 {
                continue;
            }
            let at = i as isize - q;
            if at > n {
                break;
            }
            if (0..n).contains(&at) {
                dst[at as usize] |= w >> s;
            }
            if s > 0 && (0..n).contains(&(at - 1)) {
                dst[at as usize - 1] |= w << (64 - s);
            }
        }
    }
}

/// The first clear bit of `row` below `width`, from bit `from`.
fn first_clear(row: &[u64], width: usize, from: usize) -> Option<usize> {
    let mut i = from / 64;
    let mut mask = !0u64 << (from % 64);
    while i * 64 < width {
        let mut free = !row[i] & mask;
        let end = width - i * 64;
        if end < 64 {
            free &= (1u64 << end) - 1;
        }
        if free != 0 {
            return Some(i * 64 + free.trailing_zeros() as usize);
        }
        i += 1;
        mask = !0;
    }
    None
}

/// The last clear bit of `row` below `width`.
fn last_clear(row: &[u64], width: usize) -> Option<usize> {
    let mut i = (width.max(1) - 1) / 64;
    loop {
        let mut free = !row[i];
        let end = width - i * 64;
        if end < 64 {
            free &= (1u64 << end) - 1;
        }
        if free != 0 {
            return Some(i * 64 + 63 - free.leading_zeros() as usize);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// From which corner a fill packs: the bottom or top row first, the left
/// or right end of a row first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Corner {
    pub(crate) top: bool,
    pub(crate) right: bool,
}

impl Free {
    /// A plate with nothing placed but what stays.
    pub(crate) fn new(kinds: &Kinds) -> Free {
        let maps: Vec<BitGrid> = (0..kinds.len())
            .map(|k| {
                let [w, h] = kinds.room(k);
                let mut grid = BitGrid::new(w.max(0) as usize, h.max(0) as usize);
                if let Some(held) = &kinds.held[k] {
                    // The plate is at no move: kind k's frame at `at`
                    // collides where `at` is in the map.
                    let lo = kinds.lo[k];
                    for r in 0..held.size()[1] {
                        let y = held.lo()[1] + r as i32 + lo[1];
                        if y < 0 || y >= h {
                            continue;
                        }
                        or_shifted(grid.row_mut(y as usize), held.row(r), held.lo()[0] + lo[0]);
                    }
                }
                grid
            })
            .collect();
        Free {
            first_row: vec![0; maps.len()],
            maps,
            placed: Vec::new(),
        }
    }

    /// Whether kind `k` fits at map spot `p`.
    pub(crate) fn fits(&self, kinds: &Kinds, k: usize, p: [i32; 2]) -> bool {
        let [w, h] = kinds.room(k);
        if p[0] < 0 || p[1] < 0 || p[0] >= w || p[1] >= h {
            return false;
        }
        let (x, y) = (p[0] as usize, p[1] as usize);
        self.maps[k].row(y)[x / 64] >> (x % 64) & 1 == 0
    }

    /// Puts kind `k` at map spot `p`, which must be clear.
    pub(crate) fn place(&mut self, kinds: &Kinds, k: usize, p: [i32; 2]) {
        let n = kinds.len();
        let at = kinds.at(k, p);
        for k2 in 0..n {
            let map = &kinds.nofit[k * n + k2];
            let lo2 = kinds.lo[k2];
            let h2 = self.maps[k2].height as i32;
            let shift = at[0] + map.lo()[0] + lo2[0];
            let y0 = at[1] + map.lo()[1] + lo2[1];
            let rows = map.size()[1] as i32;
            let (from, to) = ((-y0).max(0), (h2 - y0).min(rows));
            let grid = &mut self.maps[k2];
            for r in from..to {
                or_shifted(grid.row_mut((y0 + r) as usize), map.row(r as usize), shift);
            }
        }
        self.placed.push((k, p));
    }

    /// The first clear spot of kind `k` from `corner`: the nearest row
    /// to it, then the nearest end of that row.
    pub(crate) fn first(&mut self, kinds: &Kinds, k: usize, corner: Corner) -> Option<[i32; 2]> {
        let [w, h] = kinds.room(k);
        if w <= 0 || h <= 0 {
            return None;
        }
        let grid = &self.maps[k];
        let (w, h) = (w as usize, h as usize);
        let rows: Box<dyn Iterator<Item = usize>> = if corner.top {
            Box::new((0..h).rev())
        } else {
            Box::new(self.first_row[k].max(0) as usize..h)
        };
        for y in rows {
            let x = if corner.right {
                last_clear(grid.row(y), w)
            } else {
                first_clear(grid.row(y), w, 0)
            };
            if let Some(x) = x {
                if !corner.top {
                    self.first_row[k] = y as i32;
                }
                return Some([x as i32, y as i32]);
            }
        }
        if !corner.top {
            self.first_row[k] = h as i32;
        }
        None
    }
}
