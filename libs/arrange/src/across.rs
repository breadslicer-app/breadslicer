/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Arrange all over the plates, on as few as it can (#554).
//!
//! The plates are filled one after the other, each with the bodies left
//! that cover most of it, as [`crate::arrange`] packs a plate: the large
//! bodies go first, and the small ones fill around them, here or on the
//! plates after (first fit decreasing, as bin packing has it). Plates that
//! hold a body that stays are filled first: they are used anyway.
//!
//! Copies of a body share its footprint, however each stands turned (by a
//! whole number of the angles' steps), and whatever its meshes: it is
//! turned once, and the search knows them for copies. When copies make most
//! of what moves, the plates also try them in rows and on lattices, as
//! under one raft for all. A plate whose bodies are left over again (copies
//! of them, or bodies much like them that keep clear in their places) takes
//! the very layout of the plate before: the search would find it again, or
//! one a body short, which leaves the last plate with one or two bodies.
//! When the last plate holds little, its bodies are squeezed into the other
//! plates, the bodies there moving to make room, and the plate is no longer
//! needed.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::footprint::{Footprint, Peel, Shape};
use crate::plate::Occupancy;
use crate::search::{self, Problem};
use crate::separate::{self, Spot};
use crate::{
    Across, Body, Cancelled, Frame, OwnFrame, Parking, Placement, Plate, Ready, Settings, Spread,
    Timings, bands_of,
};

/// `fp`, the footprint of a body standing as `from`, for a copy of it
/// standing as `to`: the same shapes, at the same angles seen from above,
/// each reached by the copy's own turn.
fn shared(fp: &Footprint, from: &OwnFrame, to: &OwnFrame) -> Footprint {
    let d = to.turn - from.turn;
    let (s, c) = d.sin_cos();
    let turn = |p: [f64; 2]| [c * p[0] - s * p[1], s * p[0] + c * p[1]];
    let rel = turn([fp.reference[0] - from.at[0], fp.reference[1] - from.at[1]]);
    Footprint {
        reference: [to.at[0] + rel[0], to.at[1] + rel[1]],
        radius: fp.radius,
        angles: fp.angles.iter().map(|a| a - d).collect(),
        turn: fp.turn + d,
        shapes: fp.shapes.clone(),
        peel: fp
            .peel
            .iter()
            .map(|p| Peel {
                centroid: turn(p.centroid),
                ..*p
            })
            .collect(),
    }
}

/// The footprints of the bodies that move, at every angle of `frame`; `None`
/// for a body that stays or has no triangles. A copy of a body (the same
/// bands in its own frame, whatever its meshes), standing turned from it by
/// a whole number of the angles' steps, shares its shapes: they are turned
/// once, and the search knows the two for copies.
pub(crate) fn movable_footprints(frame: &Frame, bodies: &[Body]) -> Vec<Option<Footprint>> {
    let n = bodies.len();
    let step = std::f64::consts::TAU / frame.angles.len().max(1) as f64;
    let ready: Vec<Option<Ready>> = bodies
        .par_iter()
        .map(|b| if b.fixed { None } else { frame.ready(b) })
        .collect();
    let keys: Vec<Option<u64>> = ready
        .par_iter()
        .map(|r| r.as_ref().map(|r| r.base.key()))
        .collect();
    // Per body, the body whose shapes it takes.
    let mut from: Vec<Option<usize>> = vec![None; n];
    let mut firsts: HashMap<u64, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let (Some(key), Some(this)) = (keys[i], &ready[i]) else {
            continue;
        };
        let seen = firsts.entry(key).or_default();
        let alike = seen.iter().copied().find(|&r| {
            ready[r].as_ref().is_some_and(|other| {
                let d = (this.own.turn - other.own.turn) / step;
                (d - d.round()).abs() < 1e-6 && this.base.same_as(&other.base)
            })
        });
        from[i] = Some(alike.unwrap_or_else(|| {
            seen.push(i);
            i
        }));
    }
    let drawn: Vec<Option<Footprint>> = (0..n)
        .into_par_iter()
        .map(|i| {
            if from[i] != Some(i) {
                return None;
            }
            Some(frame.turned(ready[i].as_ref()?, false, &frame.angles))
        })
        .collect();
    (0..n)
        .map(|i| {
            let r = from[i]?;
            let fp = drawn[r].as_ref()?;
            if r == i {
                return Some(fp.clone());
            }
            let (to, from) = (&ready[i].as_ref()?.own, &ready[r].as_ref()?.own);
            Some(shared(fp, from, to))
        })
        .collect()
}

/// A plate as arranged.
#[derive(Clone, Debug)]
struct Filled {
    plate: usize,
    /// The plate's grid with the bodies that stay on it.
    occupancy: Occupancy,
    /// Whether nothing stays on it: the whole pile may shift.
    free: bool,
    /// The bodies that move onto it, and where each goes.
    bodies: Vec<usize>,
    spots: Vec<Spot>,
    shift: [f64; 2],
}

/// What every plate's search shares.
struct Job<'a> {
    footprints: &'a [Option<Footprint>],
    settings: &'a Settings,
    frame: &'a Frame,
    cancel: &'a AtomicBool,
    /// Each plate takes the layout that covers most, not the one with most
    /// bodies.
    by_area: bool,
    /// Copies of one body make most of what moves: the plates they take
    /// more than one of try them in rows and on lattices too, as under one
    /// raft for all.
    copies: bool,
}

impl Job<'_> {
    fn fp(&self, body: usize) -> &Footprint {
        self.footprints[body]
            .as_ref()
            .expect("a body that moves has a footprint")
    }

    /// The search's inputs for `moving` on a plate holding `occupancy`;
    /// with `rows`, copies are tried in rows and on lattices too.
    fn problem<'f>(
        &self,
        moving: &'f [Footprint],
        occupancy: &Occupancy,
        free: bool,
        rows: bool,
    ) -> Problem<'f> {
        Problem {
            footprints: moving,
            plate: occupancy.clone(),
            target: [self.frame.width as i32 / 2, self.frame.height as i32 / 2],
            cell: self.frame.geo.cell,
            origin: self.frame.origin,
            balance: free,
            passes: self.settings.passes,
            deadline: Instant::now() + self.settings.time_limit,
            tight: self.settings.tight || rows,
        }
    }

    /// Packs as many of `left` as fit on `plate`. `None` when cancelled.
    fn fill(
        &self,
        plate: usize,
        occupancy: Occupancy,
        free: bool,
        left: &[usize],
    ) -> Option<Filled> {
        let moving: Vec<Footprint> = left.iter().map(|&b| self.fp(b).clone()).collect();
        // Rows and lattices pack copies tighter, for fewer plates; bodies
        // that may all fit on this one are arranged as Arrange does.
        let rows = self.copies && self.fewest(left) > 1;
        let problem = self.problem(&moving, &occupancy, free, rows);
        let layout = search::search_by(&problem, self.cancel, self.by_area)?;
        let layout = if self.settings.squeeze {
            search::squeezed(&problem, layout, self.cancel)?
        } else {
            layout
        };
        let (bodies, spots) = left
            .iter()
            .zip(&layout.placed)
            .filter_map(|(&b, spot)| spot.map(|s| (b, s)))
            .unzip();
        Some(Filled {
            plate,
            occupancy,
            free,
            bodies,
            spots,
            shift: layout.shift,
        })
    }

    /// `last`'s layout again on `plate`, with bodies out of `left`: in each
    /// place a copy of the body there, or a body much like it (copies whose
    /// supports came out a little different) that keeps clear in that place
    /// at the same angle seen from above. `None` when a place finds none.
    fn again(&self, last: &Filled, plate: usize, left: &[usize]) -> Option<Filled> {
        let n = last.bodies.len();
        let mut taken = vec![false; left.len()];
        let mut chosen: Vec<Option<(usize, Spot)>> = vec![None; n];
        let mut occupancy = last.occupancy.clone();
        // Copies first: they stand as the bodies did.
        for (slot, (&b, &spot)) in last.bodies.iter().zip(&last.spots).enumerate() {
            let shapes = &self.fp(b).shapes;
            if let Some(k) = (0..left.len())
                .find(|&k| !taken[k] && Arc::ptr_eq(&self.fp(left[k]).shapes, shapes))
            {
                taken[k] = true;
                chosen[slot] = Some((left[k], spot));
                occupancy.stamp(&shapes[spot.0], spot.1);
            }
        }
        // Then a body much like the one there, where it keeps clear.
        for (slot, (&b, &(angle, at))) in last.bodies.iter().zip(&last.spots).enumerate() {
            if chosen[slot].is_some() {
                continue;
            }
            let like = self.fp(b);
            let shape = &like.shapes[angle];
            let size = [shape.hi[0] - shape.lo[0], shape.hi[1] - shape.lo[1]];
            let k = (0..left.len()).find(|&k| {
                if taken[k] {
                    return false;
                }
                let fp = self.fp(left[k]);
                let Some(a) = search::turned_alike(like, angle, fp) else {
                    return false;
                };
                let other = &fp.shapes[a];
                let near = [other.hi[0] - other.lo[0], other.hi[1] - other.lo[1]];
                (near[0] - size[0]).abs() <= ALIKE_CELLS
                    && (near[1] - size[1]).abs() <= ALIKE_CELLS
                    && occupancy.fits(other, at)
            })?;
            let fp = self.fp(left[k]);
            let a = search::turned_alike(like, angle, fp)?;
            taken[k] = true;
            occupancy.stamp(&fp.shapes[a], at);
            chosen[slot] = Some((left[k], (a, at)));
        }
        let (bodies, spots): (Vec<usize>, Vec<Spot>) = chosen.into_iter().flatten().unzip();
        let moving: Vec<Footprint> = bodies.iter().map(|&b| self.fp(b).clone()).collect();
        let problem = self.problem(&moving, &last.occupancy, last.free, false);
        let layout = search::settled(&problem, spots.iter().copied().map(Some).collect(), 0);
        Some(Filled {
            plate,
            bodies,
            spots,
            shift: layout.shift,
            ..last.clone()
        })
    }

    /// `filled` with `body` squeezed in, the bodies there moving to make
    /// room; `None` when it does not fit. The separator takes the bodies
    /// from the `from`-th on: plates that took the same layout are each
    /// tried another way. `Err` when cancelled.
    fn squeeze(
        &self,
        filled: &Filled,
        body: usize,
        from: usize,
    ) -> Result<Option<Filled>, Cancelled> {
        let moving: Vec<Footprint> = filled
            .bodies
            .iter()
            .chain([&body])
            .map(|&b| self.fp(b).clone())
            .collect();
        let mut placed: Vec<(usize, Spot)> = filled.spots.iter().copied().enumerate().collect();
        let n = placed.len();
        if n > 0 {
            placed.rotate_left(from % n);
        }
        let out = separate::squeeze_in(
            &moving,
            &filled.occupancy,
            placed,
            [n],
            |_| true,
            self.cancel,
        )
        .ok_or(Cancelled)?;
        if out.len() <= n {
            return Ok(None);
        }
        let mut spots: Vec<Option<Spot>> = vec![None; n + 1];
        for (k, spot) in out {
            spots[k] = Some(spot);
        }
        let problem = self.problem(&moving, &filled.occupancy, filled.free, false);
        let layout = search::settled(&problem, spots.clone(), 0);
        let mut bodies = filled.bodies.clone();
        bodies.push(body);
        Ok(Some(Filled {
            bodies,
            spots: spots.into_iter().flatten().collect(),
            shift: layout.shift,
            ..filled.clone()
        }))
    }

    /// The fewest plates `bodies` could take: in each band, the cells their
    /// shapes cover there (at the angle that covers least) over a plate's.
    fn fewest(&self, bodies: &[usize]) -> usize {
        let mut least: HashMap<*const [Shape], Vec<usize>> = HashMap::new();
        let mut per_band: Vec<usize> = Vec::new();
        for &b in bodies {
            let shapes = &self.fp(b).shapes;
            let cells = least.entry(Arc::as_ptr(shapes)).or_insert_with(|| {
                let top = shapes
                    .iter()
                    .flat_map(|s| s.bands.iter().map(|m| m.band + 1))
                    .max()
                    .unwrap_or(0);
                (0..top)
                    .map(|band| {
                        shapes
                            .iter()
                            .map(|s| {
                                s.bands
                                    .iter()
                                    .filter(|m| m.band == band)
                                    .map(|m| m.grid.count())
                                    .sum::<usize>()
                            })
                            .min()
                            .unwrap_or(0)
                    })
                    .collect()
            });
            if per_band.len() < cells.len() {
                per_band.resize(cells.len(), 0);
            }
            for (sum, c) in per_band.iter_mut().zip(cells.iter()) {
                *sum += c;
            }
        }
        let room = (self.frame.width * self.frame.height).max(1);
        per_band.iter().map(|c| c.div_ceil(room)).max().unwrap_or(0)
    }

    /// The plates with the last one's bodies squeezed into the others, the
    /// bodies there moving to make room, when they all go before `deadline`;
    /// otherwise as they were.
    fn empty_last(&self, filled: &mut Vec<Filled>, deadline: Instant) -> Result<(), Cancelled> {
        let Some(last) = filled.last() else {
            return Ok(());
        };
        if !last.free || filled.len() < 2 || last.bodies.len() > EMPTIED_AT_MOST {
            return Ok(());
        }
        let cells = |shape: &Shape| shape.bands.first().map_or(0, |m| m.grid.count());
        let size = |b: usize| self.fp(b).shapes.iter().map(cells).min().unwrap_or(0);
        let mut moving = last.bodies.clone();
        // The largest first: the one least likely to go ends it soonest.
        moving.sort_by_key(|&b| std::cmp::Reverse(size(b)));
        let room = (self.frame.width * self.frame.height) as i64;
        let mut others: Vec<Filled> = filled[..filled.len() - 1].to_vec();
        for b in moving {
            // The plates with the most room left first.
            let mut order: Vec<usize> = (0..others.len()).collect();
            order.sort_by_key(|&q| {
                let used: i64 = others[q]
                    .bodies
                    .iter()
                    .zip(&others[q].spots)
                    .map(|(&o, s)| cells(&self.fp(o).shapes[s.0]) as i64)
                    .sum();
                (used - room, q)
            });
            let mut went = false;
            for (tried, q) in order.into_iter().take(PLATES_TRIED).enumerate() {
                if Instant::now() >= deadline {
                    return Ok(());
                }
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(Cancelled);
                }
                if let Some(with) = self.squeeze(&others[q], b, q * 7)? {
                    others[q] = with;
                    went = true;
                    break;
                }
                // The plate with the most room may also be packed again,
                // with it, from scratch.
                if tried == 0 {
                    let mut all = others[q].bodies.clone();
                    all.push(b);
                    let packed = self
                        .fill(
                            others[q].plate,
                            others[q].occupancy.clone(),
                            others[q].free,
                            &all,
                        )
                        .ok_or(Cancelled)?;
                    if packed.bodies.len() == all.len() {
                        others[q] = packed;
                        went = true;
                        break;
                    }
                }
            }
            if !went {
                return Ok(());
            }
        }
        *filled = others;
        Ok(())
    }
}

/// Switches for measuring the ways to arrange against each other.
#[cfg(test)]
pub(crate) mod research {
    use std::sync::atomic::AtomicBool;

    /// Each plate takes the layout with most bodies, as before.
    pub(crate) static BY_COUNT: AtomicBool = AtomicBool::new(false);
}

/// Whether each plate takes the layout that covers most.
#[cfg(not(test))]
fn by_area() -> bool {
    true
}

#[cfg(test)]
fn by_area() -> bool {
    !research::BY_COUNT.load(Ordering::Relaxed)
}

/// How far (cells) a body may differ in size from the one it takes the
/// place of when a plate's layout is taken again.
const ALIKE_CELLS: i32 = 4;

/// Copies of a body take the plates' rows and lattices when there are at
/// least this many of them ...
const COPIES_AT_LEAST: usize = 4;

/// ... and they cover at least this share of what moves.
const COPIES_SHARE: f64 = 0.5;

/// Whether copies of one body make most of `footprints`: rows and lattices
/// pack them tighter, and cost time that a mix of bodies gains nothing for.
/// Copies whose supports came out a little different count as copies: they
/// are told apart by their size, to a few millimetres.
fn mostly_copies(footprints: &[Option<Footprint>]) -> bool {
    let area = |fp: &Footprint| {
        fp.shapes
            .first()
            .map_or(0, |s| s.bands.first().map_or(0, |m| m.grid.count()))
    };
    let kind_of = |fp: &Footprint| -> [i64; 3] {
        // The smallest box over the angles, to 8 cells, and the area, to 5 %:
        // what tells a mini from another, not a copy from a copy.
        let mut sizes: Vec<[i32; 2]> = fp
            .shapes
            .iter()
            .map(|s| [s.hi[0] - s.lo[0], s.hi[1] - s.lo[1]])
            .collect();
        sizes.sort_unstable_by_key(|z| (i64::from(z[0]) * i64::from(z[1]), *z));
        let z = sizes.first().copied().unwrap_or([0, 0]);
        let a = area(fp) as f64;
        [
            i64::from(z[0] / 8),
            i64::from(z[1] / 8),
            (a.max(1.0).ln() * 20.0).round() as i64,
        ]
    };
    let mut kinds: HashMap<[i64; 3], (usize, usize)> = HashMap::new();
    let mut all = 0;
    for fp in footprints.iter().flatten() {
        let a = area(fp);
        all += a;
        let kind = kinds.entry(kind_of(fp)).or_default();
        kind.0 += 1;
        kind.1 += a;
    }
    kinds
        .values()
        .any(|&(n, a)| n >= COPIES_AT_LEAST && a as f64 >= COPIES_SHARE * all as f64)
}

/// The last plate is emptied when it holds this many bodies at most.
const EMPTIED_AT_MOST: usize = 12;

/// The plates each body of the last plate tries, the emptiest first.
const PLATES_TRIED: usize = 2;

/// See [`crate::arrange_across`].
pub(crate) fn arrange_across(
    bodies: &[Body],
    stands_on: &[Option<usize>],
    plates: &[Plate],
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Across, Cancelled> {
    let started = Instant::now();
    let Some(first_plate) = plates.first() else {
        return Ok(Across {
            placements: vec![None; bodies.len()],
            plates_used: 0,
            spread: 0.0,
            timings: Timings::default(),
        });
    };
    let base = Frame::new(first_plate, settings);
    let footprints = movable_footprints(&base, bodies);
    // The bodies that stay, each drawn on its own plate.
    let standing: Vec<Option<Footprint>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            let plate = (*stands_on.get(i)?)?;
            if !body.fixed {
                return None;
            }
            Frame::new(plates.get(plate)?, settings).footprint(body, true, &[0.0])
        })
        .collect();
    let footprinted = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let bands = bands_of(footprints.iter().chain(&standing).flatten());
    let standing_on = |p: usize| -> Vec<&Footprint> {
        (0..bodies.len())
            .filter(|&i| stands_on.get(i).copied().flatten() == Some(p))
            .filter_map(|i| standing[i].as_ref())
            .collect()
    };
    // The plates something stays on first: they are used anyway.
    let holds: Vec<bool> = (0..plates.len())
        .map(|p| !standing_on(p).is_empty())
        .collect();
    let order: Vec<usize> = (0..plates.len())
        .filter(|&p| holds[p])
        .chain((0..plates.len()).filter(|&p| !holds[p]))
        .collect();
    let job = Job {
        footprints: &footprints,
        settings,
        frame: &base,
        cancel,
        by_area: by_area(),
        copies: mostly_copies(&footprints),
    };
    let mut left: Vec<usize> = (0..bodies.len())
        .filter(|&i| footprints[i].is_some())
        .collect();
    let mut filled: Vec<Filled> = Vec::new();
    for p in order {
        if left.is_empty() && !holds[p] {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let frame = Frame::new(&plates[p], settings);
        let fixed = standing_on(p);
        let free = fixed.is_empty();
        let same = frame.width == base.width && frame.height == base.height;
        let last_free = filled.last().filter(|f| f.free && free && same);
        let plate = match last_free.and_then(|last| job.again(last, p, &left)) {
            Some(again) => again,
            None if left.is_empty() => Filled {
                plate: p,
                occupancy: frame.plate_with(bands, fixed.iter().copied()),
                free,
                bodies: Vec::new(),
                spots: Vec::new(),
                shift: [0.0, 0.0],
            },
            None => {
                let occupancy = frame.plate_with(bands, fixed.iter().copied());
                job.fill(p, occupancy, free, &left).ok_or(Cancelled)?
            }
        };
        // An empty plate that took nothing: no plate after it will.
        if free && plate.bodies.is_empty() {
            break;
        }
        left.retain(|b| !plate.bodies.contains(b));
        filled.push(plate);
    }
    // The last plate goes when its bodies fit on the others, and the
    // plates might take them all.
    let moving: Vec<usize> = (0..bodies.len())
        .filter(|&i| footprints[i].is_some())
        .collect();
    if left.is_empty() && (holds.contains(&true) || filled.len() > job.fewest(&moving)) {
        // Bounded by the tries above; the time only guards against a very
        // slow machine.
        job.empty_last(&mut filled, Instant::now() + settings.time_limit * 2)?;
    }
    let mut placements: Vec<Option<(usize, Placement)>> = vec![None; bodies.len()];
    let mut spread = Spread::default();
    let cell = base.geo.cell;
    let mut used = 0;
    for f in &filled {
        let frame = Frame::new(&plates[f.plate], settings);
        for fp in standing_on(f.plate) {
            let at = [
                ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            spread.add(f.plate, &fp.shapes[0], at);
        }
        for (&b, &(angle, at)) in f.bodies.iter().zip(&f.spots) {
            let fp = job.fp(b);
            spread.add(f.plate, &fp.shapes[angle], at);
            placements[b] = Some((f.plate, frame.placement(fp, angle, at, f.shift)));
        }
        if !f.bodies.is_empty() || !f.free {
            used = used.max(f.plate + 1);
        }
    }
    // A body standing on a plate past those given counts it too.
    if let Some(p) = (0..bodies.len())
        .filter(|&i| standing[i].is_some())
        .filter_map(|i| stands_on[i])
        .max()
    {
        used = used.max(p + 1);
    }
    if !left.is_empty() {
        let last = used.saturating_sub(1).min(plates.len() - 1);
        let mut parking = Parking::new(&plates[last], base.geo.gap);
        for &body in &left {
            placements[body] = Some((last, parking.park(job.fp(body), cell)));
        }
        used = used.max(last + 1);
    }
    let done = Instant::now();
    Ok(Across {
        placements,
        plates_used: used,
        spread: spread.mm2(cell),
        timings: Timings {
            footprints: footprinted - started,
            search: done - footprinted,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sc_mesh_core::transform::AffineTransform3D;

    use super::*;
    use crate::Part;

    /// The last plate's bodies go onto the others when they fit there, and
    /// the plate is no longer needed; each keeps clear of the others.
    #[test]
    fn the_last_plate_is_emptied_onto_the_others() {
        let mesh = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let bodies: Vec<Body> = (0..3)
            .map(|k| Body {
                parts: vec![Part {
                    mesh: mesh.clone(),
                    world: AffineTransform3D::from_linear_and_translation(
                        [[20.0, 0.0, 0.0], [0.0, 20.0, 0.0], [0.0, 0.0, 5.0]],
                        [200.0 + 30.0 * f64::from(k), 0.0, 2.5],
                    ),
                }],
                fixed: false,
            })
            .collect();
        let plate = Plate {
            min: [0.0, 0.0],
            max: [60.0, 60.0],
            floor: 0.0,
        };
        let settings = Settings {
            turns: 1,
            ..Settings::default()
        };
        let frame = Frame::new(&plate, &settings);
        let footprints = movable_footprints(&frame, &bodies);
        let cancel = AtomicBool::new(false);
        let job = Job {
            footprints: &footprints,
            settings: &settings,
            frame: &frame,
            cancel: &cancel,
            by_area: true,
            copies: false,
        };
        let empty = frame.plate_with(bands_of(footprints.iter().flatten()), []);
        let mut filled = vec![
            job.fill(0, empty.clone(), true, &[0, 1]).expect("filled"),
            job.fill(1, empty.clone(), true, &[2]).expect("filled"),
        ];
        job.empty_last(&mut filled, Instant::now() + Duration::from_secs(60))
            .expect("not cancelled");
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0].bodies.len(), 3);
        let mut on = empty;
        for (&b, &(angle, at)) in filled[0].bodies.iter().zip(&filled[0].spots) {
            let shape = &job.fp(b).shapes[angle];
            assert!(on.fits(shape, at), "body {b} keeps clear");
            on.stamp(shape, at);
        }
    }
}
