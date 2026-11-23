/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Copies of one body on a repeating pattern (Fill bed).
//!
//! Copies of one shape pack densest on a lattice: every copy turned alike,
//! at `i·u + j·v`. A shape that is not symmetric packs denser still in
//! pairs, a copy and one turned half a turn nested against it, the pair
//! repeated on a lattice. This finds such patterns and lays the one that
//! puts the most copies on the plate.
//!
//! Where two shapes collide is worked out once per pair of shapes as a
//! no-fit map: the moves of one, relative to the other, at which they
//! share a cell in some band. Each band's rows are runs of cells, and two
//! runs meet over an interval of moves, so the map is drawn run by run,
//! exactly. A pattern then only looks moves up in it. Where a pattern
//! sits on the plate is tried at every offset, counting the copies that
//! lie on the plate and clear of what stays there.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::footprint::Shape;
use crate::grid::BitGrid;
use crate::nest::{Free, Kinds};
use crate::plate::Occupancy;

/// A shape's cells as runs, band by band: per band, its rows (plate cell
/// `y` in the body's frame) and their runs `[x0, x1)`.
/// A band's rows: each row's `y` and its runs `[x0, x1)`.
type Rows = Vec<(i32, Vec<[i32; 2]>)>;

#[derive(Debug)]
pub(crate) struct Runs {
    bands: Vec<(usize, Rows)>,
    /// Per band, the larger bands that hold all of it: where it collides,
    /// they do.
    inside: Vec<Vec<usize>>,
    lo: [i32; 2],
    hi: [i32; 2],
}

/// Whether every run of `small` lies in a run of `large`, row by row.
fn holds_all(large: &Rows, small: &Rows) -> bool {
    let mut j = 0;
    for (y, runs) in small {
        while j < large.len() && large[j].0 < *y {
            j += 1;
        }
        if j >= large.len() || large[j].0 != *y {
            return false;
        }
        let big = &large[j].1;
        let mut k = 0;
        for r in runs {
            while k < big.len() && big[k][1] <= r[0] {
                k += 1;
            }
            if k >= big.len() || big[k][0] > r[0] || big[k][1] < r[1] {
                return false;
            }
        }
    }
    true
}

/// Per band of `bands`, the bands that hold it and are larger (more cells,
/// or as many and lower).
fn insides(bands: &[(usize, Rows)]) -> Vec<Vec<usize>> {
    let cells: Vec<i64> = bands
        .iter()
        .map(|(_, rows)| {
            rows.iter()
                .flat_map(|(_, r)| r.iter().map(|[a, b]| i64::from(b - a)))
                .sum()
        })
        .collect();
    let bounds: Vec<Option<[i32; 4]>> = bands
        .iter()
        .map(|(_, rows)| {
            let first = rows.first()?;
            let last = rows.last()?;
            let x0 = rows.iter().map(|(_, r)| r[0][0]).min()?;
            let x1 = rows.iter().map(|(_, r)| r[r.len() - 1][1]).max()?;
            Some([x0, first.0, x1, last.0 + 1])
        })
        .collect();
    (0..bands.len())
        .map(|x| {
            let Some(bx) = bounds[x] else {
                return Vec::new();
            };
            (0..bands.len())
                .filter(|&c| {
                    c != x
                        && (cells[c] > cells[x] || (cells[c] == cells[x] && c < x))
                        && bounds[c].is_some_and(|bc| {
                            bc[0] <= bx[0] && bc[1] <= bx[1] && bc[2] >= bx[2] && bc[3] >= bx[3]
                        })
                        && holds_all(&bands[c].1, &bands[x].1)
                })
                .map(|c| bands[c].0)
                .collect()
        })
        .collect()
}

impl Runs {
    pub(crate) fn of(shape: &Shape) -> Runs {
        let mut bands: Vec<(usize, Rows)> = shape
            .bands
            .iter()
            .map(|m| {
                let rows = (0..m.grid.height)
                    .filter_map(|y| {
                        let runs = m.grid.runs(y);
                        (!runs.is_empty()).then(|| {
                            (
                                m.y0 + y as i32,
                                runs.into_iter()
                                    .map(|(a, b)| [m.x0 + a as i32, m.x0 + b as i32])
                                    .collect(),
                            )
                        })
                    })
                    .collect();
                (m.band, rows)
            })
            .collect();
        bands.sort_by_key(|(b, _)| *b);
        let inside = insides(&bands);
        Runs {
            bands,
            inside,
            lo: shape.lo,
            hi: shape.hi,
        }
    }

    /// What the plate holds, as runs in plate cells.
    pub(crate) fn of_plate(plate: &Occupancy) -> Runs {
        let mut bands = Vec::new();
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for (band, grid) in plate.grids() {
            let rows: Vec<(i32, Vec<[i32; 2]>)> = (0..grid.height)
                .filter_map(|y| {
                    let runs = grid.runs(y);
                    (!runs.is_empty()).then(|| {
                        (
                            y as i32,
                            runs.into_iter()
                                .map(|(a, b)| [a as i32, b as i32])
                                .collect(),
                        )
                    })
                })
                .collect();
            for (y, runs) in &rows {
                lo = [lo[0].min(runs[0][0]), lo[1].min(*y)];
                hi = [hi[0].max(runs[runs.len() - 1][1]), hi[1].max(y + 1)];
            }
            if !rows.is_empty() {
                bands.push((band, rows));
            }
        }
        let inside = vec![Vec::new(); bands.len()];
        Runs {
            bands,
            inside,
            lo,
            hi,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    fn band(&self, band: usize) -> Option<(usize, &Rows)> {
        self.bands
            .binary_search_by_key(&band, |(b, _)| *b)
            .ok()
            .map(|k| (k, &self.bands[k].1))
    }
}

/// The moves `d` of a shape `b` from a shape `a` at which the two share a
/// cell in some band: `b` with its frame's corner at `a`'s plus `d`
/// collides with `a`.
#[derive(Clone, Debug)]
pub(crate) struct Nofit {
    lo: [i32; 2],
    hi: [i32; 2],
    grid: BitGrid,
}

impl Nofit {
    pub(crate) fn of(a: &Runs, b: &Runs) -> Nofit {
        let lo = [a.lo[0] - b.hi[0] + 1, a.lo[1] - b.hi[1] + 1];
        let hi = [a.hi[0] - b.lo[0], a.hi[1] - b.lo[1]];
        let size = [
            (hi[0] - lo[0]).max(0) as usize,
            (hi[1] - lo[1]).max(0) as usize,
        ];
        let mut grid = BitGrid::new(size[0], size[1]);
        for (ia, (band, rows_a)) in a.bands.iter().enumerate() {
            let Some((ib, rows_b)) = b.band(*band) else {
                continue;
            };
            // A band both shapes hold in a larger band of theirs collides
            // only where that band does.
            if a.inside[ia].iter().any(|c| b.inside[ib].contains(c)) {
                continue;
            }
            for (ya, ra) in rows_a {
                for (yb, rb) in rows_b {
                    let row = (ya - yb - lo[1]) as usize;
                    for &[a0, a1] in ra {
                        for &[b0, b1] in rb {
                            // a0 + x = b0 + d for some x, and so on: d runs
                            // from a0 - (b1 - 1) to (a1 - 1) - b0.
                            let x0 = a0 - b1 + 1 - lo[0];
                            let x1 = a1 - 1 - b0 - lo[0];
                            grid.set_span(row, x0 as usize, x1 as usize);
                        }
                    }
                }
            }
        }
        Nofit { lo, hi, grid }
    }

    /// The moves of `a` from `b` at which they collide: these, turned
    /// half a turn.
    pub(crate) fn reflected(&self) -> Nofit {
        let [w, h] = [self.grid.width, self.grid.height];
        let mut grid = BitGrid::new(w, h);
        for y in 0..h {
            let src = self.grid.row(h - 1 - y);
            let dst = grid.row_mut(y);
            // Bit x of the source row lands on bit w - 1 - x.
            let words = src.len();
            let pad = words * 64 - w;
            for (i, &word) in src.iter().enumerate() {
                if word == 0 {
                    continue;
                }
                let r = word.reverse_bits();
                // Reversed within the row's words, then moved down by the
                // padding past the row's end.
                let at = (words - 1 - i) * 64;
                let lo = at as isize - pad as isize;
                if lo >= 0 {
                    let (q, s) = (lo as usize / 64, lo as usize % 64);
                    dst[q] |= r << s;
                    if s > 0 && q + 1 < words {
                        dst[q + 1] |= r >> (64 - s);
                    }
                } else {
                    dst[0] |= r >> ((-lo) as usize);
                }
            }
        }
        Nofit {
            lo: [1 - self.hi[0], 1 - self.hi[1]],
            hi: [1 - self.lo[0], 1 - self.lo[1]],
            grid,
        }
    }

    /// The first move it spans.
    pub(crate) fn lo(&self) -> [i32; 2] {
        self.lo
    }

    /// Row `r` of moves, from [`Self::lo`].
    pub(crate) fn row(&self, r: usize) -> &[u64] {
        self.grid.row(r)
    }

    /// How many moves it spans, across and up.
    pub(crate) fn size(&self) -> [usize; 2] {
        [self.grid.width, self.grid.height]
    }

    /// Whether `b` moved by `d` from `a` collides with it.
    pub(crate) fn hit(&self, d: [i32; 2]) -> bool {
        if d[0] < self.lo[0] || d[1] < self.lo[1] || d[0] >= self.hi[0] || d[1] >= self.hi[1] {
            return false;
        }
        let (x, y) = ((d[0] - self.lo[0]) as usize, (d[1] - self.lo[1]) as usize);
        self.grid.row(y)[x / 64] >> (x % 64) & 1 == 1
    }
}

/// What a repeated cluster collides at: the moves of a copy of the cluster
/// from another at which some of their members meet.
struct Cluster<'a> {
    /// Same angle against same angle, and the second angle against itself.
    aa: &'a Nofit,
    bb: Option<&'a Nofit>,
    /// The second member, at `t` from the first, and the first against it.
    ab: Option<(&'a Nofit, [i32; 2])>,
    /// Where any hit can lie: a box of moves.
    lo: [i32; 2],
    hi: [i32; 2],
}

impl Cluster<'_> {
    fn hit(&self, d: [i32; 2]) -> bool {
        if d[0] < self.lo[0] || d[1] < self.lo[1] || d[0] >= self.hi[0] || d[1] >= self.hi[1] {
            return false;
        }
        if self.aa.hit(d) || self.bb.is_some_and(|bb| bb.hit(d)) {
            return true;
        }
        match self.ab {
            // The first of one cluster against the second of the other,
            // both ways.
            Some((ab, t)) => {
                ab.hit([d[0] + t[0], d[1] + t[1]]) || ab.hit([t[0] - d[0], t[1] - d[1]])
            }
            None => false,
        }
    }
}

trait IntoI32 {
    fn into_i32(self) -> (i32, i32);
}

impl IntoI32 for (i64, i64) {
    fn into_i32(self) -> (i32, i32) {
        let c = |x: i64| x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        (c(self.0), c(self.1))
    }
}

fn det(u: [i32; 2], v: [i32; 2]) -> i64 {
    i64::from(u[0]) * i64::from(v[1]) - i64::from(u[1]) * i64::from(v[0])
}

/// How many directions the steps across a row are tried in, over half a
/// turn.
const DIRECTIONS: usize = 36;

/// How many lattices per cluster are tried on the plate, the densest first.
const LATTICES: usize = 8;

/// The densest lattices a cluster repeats on without colliding, densest
/// first: steps `u` across a row, in every direction, each with the step
/// `v` to the next row that packs the rows closest.
fn lattices(cluster: &Cluster<'_>) -> Vec<([i32; 2], [i32; 2], i64)> {
    let corners = [
        cluster.lo,
        [cluster.hi[0], cluster.lo[1]],
        [cluster.lo[0], cluster.hi[1]],
        cluster.hi,
    ];
    let reach = corners
        .iter()
        .map(|c| f64::from(c[0]).hypot(f64::from(c[1])))
        .fold(0.0, f64::max);
    let mut us: Vec<[i32; 2]> = Vec::new();
    for k in 0..DIRECTIONS {
        let phi = std::f64::consts::PI * k as f64 / DIRECTIONS as f64;
        let (s, c) = phi.sin_cos();
        let mut step = 1.0;
        while step <= reach + 2.0 {
            let u = [(c * step).round() as i32, (s * step).round() as i32];
            let ks = (reach / step).ceil() as i32 + 1;
            if u != [0, 0] && (1..=ks).all(|k| !cluster.hit([k * u[0], k * u[1]])) {
                if !us.contains(&u) {
                    us.push(u);
                }
                break;
            }
            step += 1.0;
        }
    }
    let mut found: Vec<([i32; 2], [i32; 2], i64)> = us
        .par_iter()
        .filter_map(|&u| {
            let uu = i64::from(u[0]) * i64::from(u[0]) + i64::from(u[1]) * i64::from(u[1]);
            let ulen = (uu as f64).sqrt();
            // How far from the line through u a hit can lie, as a det.
            let far = corners.iter().map(|&c| det(u, c).abs()).max().unwrap_or(0) + 1;
            let ks = (reach / ulen).ceil() as i32 + 2;
            // Nearest the row first: a hit shows there soonest.
            let order: Vec<i32> = (0..=2 * ks)
                .map(|i| if i % 2 == 0 { i / 2 } else { -(i + 1) / 2 })
                .collect();
            let valid = |v: [i32; 2]| {
                let d = det(u, v);
                let mut j = 1;
                while i64::from(j) * d <= far {
                    for &k in &order {
                        if cluster.hit([j * v[0] + k * u[0], j * v[1] + k * u[1]]) {
                            return false;
                        }
                    }
                    j += 1;
                }
                true
            };
            let r = reach.ceil() as i32 + 2;
            let mut best: Option<([i32; 2], i64)> = None;
            for vy in -r..=r {
                // The v of this row with 0 <= v·u < u·u: one of each class
                // v + k·u.
                let rest = i64::from(vy) * i64::from(u[1]);
                let (x0, x1) = match u[0].signum() {
                    0 if (0..uu).contains(&rest) => (-r, r),
                    0 => continue,
                    1 => (
                        (-rest).div_euclid(i64::from(u[0]))
                            + i64::from((-rest).rem_euclid(i64::from(u[0])) != 0),
                        (uu - rest - 1).div_euclid(i64::from(u[0])),
                    )
                        .into_i32(),
                    _ => {
                        let ux = -i64::from(u[0]);
                        (
                            (rest - uu + 1).div_euclid(ux)
                                + i64::from((rest - uu + 1).rem_euclid(ux) != 0),
                            rest.div_euclid(ux),
                        )
                            .into_i32()
                    }
                };
                for vx in x0.max(-r)..=x1.min(r) {
                    let v = [vx, vy];
                    let d = det(u, v);
                    if d <= 0 || best.is_some_and(|(_, b)| d >= b) {
                        continue;
                    }
                    if valid(v) {
                        best = Some((v, d));
                    }
                }
            }
            best.map(|(v, d)| (u, v, d))
        })
        .collect();
    found.sort_by_key(|&(u, v, d)| (d, u, v));
    found.truncate(LATTICES);
    found
}

/// A member of a pattern: its kind, its move from the cluster's corner, and
/// where its frame's corner may go on the plate, `[lo, hi]`.
struct Member {
    kind: usize,
    t: [i32; 2],
    room: ([i32; 2], [i32; 2]),
}

impl Member {
    fn new(kinds: &Kinds, kind: usize, t: [i32; 2]) -> Member {
        let lo = kinds.lo(kind);
        let room = kinds.room(kind);
        Member {
            kind,
            t,
            room: ([-lo[0], -lo[1]], [room[0] - 1 - lo[0], room[1] - 1 - lo[1]]),
        }
    }

    fn fits(&self, kinds: &Kinds, free: &Free, at: [i32; 2]) -> bool {
        let lo = kinds.lo(self.kind);
        free.fits(kinds, self.kind, [at[0] + lo[0], at[1] + lo[1]])
    }
}

/// Calls `f` with each lattice point `o + i·u + j·v` whose member fits.
fn each_laid(
    kinds: &Kinds,
    free: &Free,
    members: &[Member],
    u: [i32; 2],
    v: [i32; 2],
    o: [i32; 2],
    mut f: impl FnMut(usize, [i32; 2]),
) {
    let d = det(u, v) as f64;
    for (k, m) in members.iter().enumerate() {
        let (lo, hi) = m.room;
        // The room's corners, in lattice coordinates, bound i and j.
        let (mut ilo, mut ihi, mut jlo, mut jhi) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for c in [lo, [hi[0], lo[1]], [lo[0], hi[1]], hi] {
            let p = [
                f64::from(c[0] - o[0] - m.t[0]),
                f64::from(c[1] - o[1] - m.t[1]),
            ];
            let i = (p[0] * f64::from(v[1]) - p[1] * f64::from(v[0])) / d;
            let j = (p[1] * f64::from(u[0]) - p[0] * f64::from(u[1])) / d;
            ilo = ilo.min(i.floor() as i32);
            ihi = ihi.max(i.ceil() as i32);
            jlo = jlo.min(j.floor() as i32);
            jhi = jhi.max(j.ceil() as i32);
        }
        for j in jlo..=jhi {
            for i in ilo..=ihi {
                let at = [
                    o[0] + m.t[0] + i * u[0] + j * v[0],
                    o[1] + m.t[1] + i * u[1] + j * v[1],
                ];
                if at[0] >= lo[0]
                    && at[1] >= lo[1]
                    && at[0] <= hi[0]
                    && at[1] <= hi[1]
                    && m.fits(kinds, free, at)
                {
                    f(k, at);
                }
            }
        }
    }
}

/// The offset of the lattice `(u, v)` that fits most members, and how many.
fn best_offset(
    kinds: &Kinds,
    free: &Free,
    members: &[Member],
    u: [i32; 2],
    v: [i32; 2],
) -> ([i32; 2], usize) {
    // One offset of each class of the lattice: x below a, y below c, where
    // c spans the rows' heights and a·c the lattice's area.
    let c = gcd(u[1], v[1]).max(1);
    let a = (det(u, v).abs() / i64::from(c)).max(1) as i32;
    let count = |o: [i32; 2]| {
        let mut n = 0;
        each_laid(kinds, free, members, u, v, o, |_, _| n += 1);
        n
    };
    let coarse = |n: i32| ((f64::from(n) / OFFSETS).ceil() as i32).max(1);
    let (sx, sy) = (coarse(a), coarse(c));
    let mut best = ([0, 0], 0);
    let mut y = 0;
    while y < c {
        let mut x = 0;
        while x < a {
            let n = count([x, y]);
            if n > best.1 {
                best = ([x, y], n);
            }
            x += sx;
        }
        y += sy;
    }
    // Then every offset around the best coarse one.
    let around = best.0;
    for dy in -sy..=sy {
        for dx in -sx..=sx {
            let o = [around[0] + dx, around[1] + dy];
            let n = count(o);
            if n > best.1 {
                best = (o, n);
            }
        }
    }
    best
}

/// How many offsets a lattice is tried at, coarsely, along each of its
/// two steps.
const OFFSETS: f64 = 32.0;

fn gcd(a: i32, b: i32) -> i32 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A lattice on the plate: its steps `u` and `v`, its offset, and how many
/// copies it lays there.
type Lattice = ([i32; 2], [i32; 2], [i32; 2], usize);

/// How many moves of the second of a pair are tried against the first.
const PAIRS: usize = 3;

/// The copies a pattern lays: kind and frame's corner, front row first.
pub(crate) type Laid = Vec<(usize, [i32; 2])>;

/// The patterns copies of one body (its kinds, one per angle, in `body`)
/// can stand on, on the plate of `free`, each with the most copies it lays
/// there, the one with most first. `None` when cancelled.
pub(crate) fn patterns(
    kinds: &Kinds,
    free: &Free,
    body: &[usize],
    cancel: &AtomicBool,
) -> Option<Vec<Laid>> {
    let n = body.len();
    // Every cluster: one copy at each angle, and pairs at each angle with
    // one turned half a turn from it.
    let mut clusters: Vec<(Vec<Member>, Cluster<'_>)> = Vec::new();
    for a in 0..n {
        let ka = body[a];
        let aa = kinds.nofit(ka, ka);
        clusters.push((
            vec![Member::new(kinds, ka, [0, 0])],
            Cluster {
                aa,
                bb: None,
                ab: None,
                lo: aa.lo,
                hi: aa.hi,
            },
        ));
        if !n.is_multiple_of(2) || n < 2 {
            continue;
        }
        let kb = body[(a + n / 2) % n];
        let ab = kinds.nofit(ka, kb);
        let bb = kinds.nofit(kb, kb);
        // The second as close to the first as it comes, in each direction:
        // the closest few.
        let mut ts: Vec<[i32; 2]> = Vec::new();
        for k in 0..36 {
            let phi = std::f64::consts::TAU * f64::from(k) / 36.0;
            let (s, c) = phi.sin_cos();
            let mut step = 1.0;
            loop {
                let t = [(c * step).round() as i32, (s * step).round() as i32];
                if !ab.hit(t) {
                    if !ts.contains(&t) {
                        ts.push(t);
                    }
                    break;
                }
                step += 1.0;
            }
        }
        ts.sort_by_key(|t| i64::from(t[0]) * i64::from(t[0]) + i64::from(t[1]) * i64::from(t[1]));
        ts.truncate(PAIRS);
        for t in ts {
            let lo = [
                aa.lo[0]
                    .min(bb.lo[0])
                    .min(ab.lo[0] - t[0])
                    .min(t[0] - ab.hi[0] + 1),
                aa.lo[1]
                    .min(bb.lo[1])
                    .min(ab.lo[1] - t[1])
                    .min(t[1] - ab.hi[1] + 1),
            ];
            let hi = [
                aa.hi[0]
                    .max(bb.hi[0])
                    .max(ab.hi[0] - t[0])
                    .max(t[0] - ab.lo[0] + 1),
                aa.hi[1]
                    .max(bb.hi[1])
                    .max(ab.hi[1] - t[1])
                    .max(t[1] - ab.lo[1] + 1),
            ];
            clusters.push((
                vec![Member::new(kinds, ka, [0, 0]), Member::new(kinds, kb, t)],
                Cluster {
                    aa,
                    bb: Some(bb),
                    ab: Some((ab, t)),
                    lo,
                    hi,
                },
            ));
        }
    }
    let laid: Vec<Option<Laid>> = clusters
        .par_iter()
        .map(|(members, cluster)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // The steps, the offset, and how many it lays.
            let mut best: Option<Lattice> = None;
            for (u, v, _) in lattices(cluster) {
                let (o, count) = best_offset(kinds, free, members, u, v);
                if best.is_none_or(|b| count > b.3) {
                    best = Some((u, v, o, count));
                }
            }
            let (u, v, o, _) = best?;
            let mut spots: Laid = Vec::new();
            each_laid(kinds, free, members, u, v, o, |k, at| {
                spots.push((members[k].kind, at));
            });
            spots.sort_by_key(|&(k, at)| {
                let lo = kinds.lo(k);
                (at[1] + lo[1], at[0] + lo[0])
            });
            Some(spots)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut laid: Vec<Laid> = laid.into_iter().flatten().collect();
    laid.sort_by_key(|l| std::cmp::Reverse(l.len()));
    Some(laid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-fit map turned half a turn is the map of the pair the other
    /// way round.
    #[test]
    fn a_reflected_map_is_the_other_way_round() {
        let mut grid = BitGrid::new(70, 3);
        grid.set_span(0, 0, 0);
        grid.set_span(1, 3, 66);
        grid.set_span(2, 69, 69);
        let map = Nofit {
            lo: [-5, 2],
            hi: [65, 5],
            grid,
        };
        let back = map.reflected();
        for y in -10..10 {
            for x in -80..80 {
                assert_eq!(map.hit([x, y]), back.hit([-x, -y]), "{x} {y}");
            }
        }
    }
}
