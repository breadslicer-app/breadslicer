/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Fills the plate with copies (#356).
//!
//! Every body but the sources stays where it is, and the others keep clear
//! of it. Copies of the sources go in the room left, as many as fit: a
//! copy of each source in turn, so the sources get as many copies each,
//! give or take one. A copy is its source moved in the plate plane, maybe
//! turned about the vertical axis, as [`crate::arrange`] moves a body; it
//! keeps the same gap and edge margin.
//!
//! The sources stay where they stand too, unless moving them lets more
//! copies in: a source alone in the middle of a plate splits it, and rows
//! that would fit side by side no longer do. Both ways are searched, and
//! the one with more copies wins; on a tie the sources stay. A source that
//! moves takes the place nearest where it stood.
//!
//! With a few sources, every angle of each is weighed against every other
//! once, as a no-fit map ([`crate::nest`]): placing a copy is then a few
//! bit operations, and a fill takes well under a millisecond. Thousands of
//! fills run ([`crate::packing`]): from each corner of the plate, at one
//! angle, at one and half a turn from it, or at any; on the patterns one
//! copy, or a pair nested half a turn apart, repeats on
//! ([`crate::pattern`]); and as pilots that weigh each copy's angle by
//! filling the rest of the plate from there. The quick passes of Arrange
//! all ([`crate::search`]) run too. The two best fills then get more copies
//! pressed in ([`crate::press`]), and the one with most wins. With more
//! sources, the passes alone run, and their best gets more copies squeezed
//! in ([`crate::separate`]).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::footprint::{Footprint, Shape};
use crate::nest::{Free, Kinds};
use crate::press::Depths;
use crate::search::Problem;
use crate::{Body, Cancelled, Frame, Placement, Plate, Settings, Timings, bands_of, search};

/// A copy [`fill`] found room for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillCopy {
    /// The body it copies, by its index among the bodies.
    pub source: usize,
    /// Where the copy goes: its source's move, from where the source
    /// stands.
    pub placement: Placement,
}

/// What [`fill`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    /// The copies, in the order they were placed: one of each source in
    /// turn.
    pub copies: Vec<FillCopy>,
    /// Where each source goes, when moving them lets more copies in; empty
    /// when they stay where they stand.
    pub moved: Vec<FillCopy>,
    pub timings: Timings,
}

/// Finds room on `plate` for copies of the bodies `sources` (indices into
/// `bodies`), around every other body, which stays where it is: as many as
/// fit, or `limit` at most. The sources stay too, or move when that lets
/// more copies in ([`Filled::moved`]). `placed` follows how many the search has
/// placed so far, for a progress report. Checks `cancel` as it goes.
pub fn fill(
    bodies: &[Body],
    sources: &[usize],
    plate: &Plate,
    settings: &Settings,
    limit: Option<usize>,
    placed: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Filled, Cancelled> {
    let started = Instant::now();
    let frame = Frame::new(plate, settings);
    let mut sources: Vec<usize> = sources
        .iter()
        .copied()
        .filter(|&i| i < bodies.len())
        .collect();
    sources.sort_unstable();
    sources.dedup();
    // A source's footprint turns, and its first angle, the way it stands,
    // is also where it stands.
    let footprints: Vec<Option<Footprint>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            if sources.binary_search(&i).is_ok() {
                frame.footprint(body, true, &frame.angles)
            } else {
                frame.footprint(body, true, &[0.0])
            }
        })
        .collect();
    let footprinted = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let bands = bands_of(footprints.iter().flatten());
    // Sources with no triangles have nothing to copy.
    let sources: Vec<usize> = sources
        .into_iter()
        .filter(|&i| footprints[i].is_some())
        .collect();
    let copied: Vec<Footprint> = sources
        .iter()
        .filter_map(|&i| footprints[i].clone())
        .collect();
    let n = sources.len();
    // With few sources, every angle of every source against every other is
    // drawn once as a no-fit map, and the fills run on them ([`crate::nest`]):
    // unless the plate holds only a few, where the maps of such large
    // shapes take longer than they save.
    let plate_cells = frame.width as f64 * frame.height as f64;
    let few = copied.iter().all(|fp| {
        let smallest = fp.shapes.iter().map(Shape::area).min().unwrap_or(0);
        smallest > 0 && plate_cells / (smallest as f64) < FEW
    });
    let nested = n <= NESTED && !few;
    let problem = |plate| Problem {
        footprints: &copied,
        plate,
        target: [frame.width as i32 / 2, frame.height as i32 / 2],
        cell: frame.geo.cell,
        origin: frame.origin,
        balance: false,
        passes: 0,
        deadline: started,
        // On the maps, the patterns copies can stand on are tried anyway.
        tight: settings.tight && !nested,
    };
    let staying = problem(frame.plate_with(bands, footprints.iter().flatten()));
    let others = footprints
        .iter()
        .enumerate()
        .filter(|(i, _)| sources.binary_search(i).is_err())
        .filter_map(|(_, fp)| fp.as_ref());
    let moving = problem(frame.plate_with(bands, others));
    let shapes: Vec<&[Shape]> = copied.iter().map(|fp| &fp.shapes[..]).collect();
    // The quick passes on both plates, as before; meanwhile the maps.
    let ((stayed, moved), maps) = rayon::join(
        || {
            rayon::join(
                || search::fill(&staying, limit, placed, 0, cancel),
                || search::fill(&moving, limit.map(|l| l + n), placed, n, cancel),
            )
        },
        || {
            nested.then(|| {
                let staying_kinds = Kinds::new(&shapes, &staying.plate);
                let moving_kinds = staying_kinds.on(&moving.plate);
                (staying_kinds, moving_kinds)
            })
        },
    );
    let (stayed, moved) = (stayed.ok_or(Cancelled)?, moved.ok_or(Cancelled)?);
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    // With the sources moving, every source must find a place of its own.
    let all_in = |copies: &[search::Copied]| (0..n).all(|k| copies.iter().any(|c| c.0 == k));
    let (moving_wins, copies) = match &maps {
        None => {
            let moving_wins = all_in(&moved) && moved.len() - n > stayed.len();
            // More copies squeezed in, the copies placed making room (#555).
            let copies = if moving_wins {
                squeezed(
                    &moving,
                    moved,
                    limit.map(|l| l + n),
                    settings.squeeze,
                    cancel,
                )
            } else {
                squeezed(&staying, stayed, limit, settings.squeeze, cancel)
            }
            .ok_or(Cancelled)?;
            (moving_wins, copies)
        }
        Some((staying_kinds, moving_kinds)) => {
            let size = [frame.width as i32, frame.height as i32];
            let packed = |kinds: &Kinds, limit: Option<usize>| {
                let free = Free::new(kinds);
                crate::packing::pack(kinds, &free, 0, limit.unwrap_or(usize::MAX), size, cancel)
            };
            let (packed_staying, packed_moving) = rayon::join(
                || packed(staying_kinds, limit),
                || packed(moving_kinds, limit.map(|l| l + n)),
            );
            let (packed_staying, packed_moving) = (
                packed_staying.ok_or(Cancelled)?,
                packed_moving.ok_or(Cancelled)?,
            );
            // Each plate's fills, the one with most first; the maps' on a
            // tie.
            let ranked = |fills: [Vec<search::Copied>; 2]| {
                let mut fills: Vec<Vec<search::Copied>> = fills.into_iter().collect();
                fills.sort_by_key(|f| std::cmp::Reverse(f.len()));
                fills.dedup();
                fills
            };
            let on_staying = ranked([packed_staying, stayed]);
            let on_moving: Vec<Vec<search::Copied>> = ranked([packed_moving, moved])
                .into_iter()
                .filter(|f| all_in(f))
                .collect();
            let most = |fills: &[Vec<search::Copied>]| fills.first().map_or(0, Vec::len);
            let moving_wins = most(&on_moving) > n && most(&on_moving) - n > most(&on_staying);
            let (kinds, fills, limit, uncounted) = if moving_wins {
                (moving_kinds, on_moving, limit.map(|l| l + n), n)
            } else {
                (staying_kinds, on_staying, limit, 0)
            };
            placed.fetch_max(most(&fills).saturating_sub(uncounted), Ordering::Relaxed);
            let copies = if settings.squeeze && Depths::fit(kinds) {
                // Then more copies pressed in, the copies placed making
                // room: into each plate's fills, and the one that ends
                // with most wins.
                let depths = Depths::of(kinds);
                let pressed: Vec<Option<Vec<search::Copied>>> = fills
                    .into_par_iter()
                    .map(|fill| {
                        if fill.is_empty() {
                            return Some(fill);
                        }
                        let start = fill.len();
                        crate::press::press_in(
                            kinds,
                            &depths,
                            &Free::new(kinds),
                            fill,
                            (start..).map(|c| c % n),
                            |count| {
                                placed
                                    .fetch_max(count.saturating_sub(uncounted), Ordering::Relaxed);
                                limit.is_none_or(|l| count < l)
                            },
                            cancel,
                        )
                    })
                    .collect();
                let pressed: Option<Vec<Vec<search::Copied>>> = pressed.into_iter().collect();
                let mut pressed = pressed.ok_or(Cancelled)?;
                let best = (0..pressed.len())
                    .max_by(|&a, &b| pressed[a].len().cmp(&pressed[b].len()).then(b.cmp(&a)))
                    .unwrap_or(0);
                if pressed.is_empty() {
                    Vec::new()
                } else {
                    pressed.swap_remove(best)
                }
            } else {
                fills.into_iter().next().unwrap_or_default()
            };
            (moving_wins, copies)
        }
    };
    placed.fetch_max(
        copies.len().saturating_sub(if moving_wins { n } else { 0 }),
        Ordering::Relaxed,
    );
    let to_fill = |(k, angle, at): search::Copied| FillCopy {
        source: sources[k],
        placement: frame.placement(&copied[k], angle, at, [0.0, 0.0]),
    };
    let (copies, moved): (Vec<FillCopy>, Vec<FillCopy>) = if moving_wins {
        // Each source moves to its copy nearest where it stands, so it
        // moves as little as it can.
        let cell = frame.geo.cell;
        let own: Vec<usize> = (0..n)
            .map(|k| {
                let fp = &copied[k];
                let here = [
                    ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                    ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
                ];
                (0..copies.len())
                    .filter(|&c| copies[c].0 == k)
                    .min_by_key(|&c| {
                        let at = copies[c].2;
                        let d = [i64::from(at[0] - here[0]), i64::from(at[1] - here[1])];
                        (d[0] * d[0] + d[1] * d[1], c)
                    })
                    .unwrap_or(usize::MAX)
            })
            .collect();
        let originals = own.iter().map(|&c| to_fill(copies[c])).collect();
        let copies = copies
            .into_iter()
            .enumerate()
            .filter(|(c, _)| !own.contains(c))
            .map(|(_, found)| to_fill(found))
            .collect();
        (copies, originals)
    } else {
        (copies.into_iter().map(to_fill).collect(), Vec::new())
    };
    Ok(Filled {
        copies,
        moved,
        timings: Timings {
            footprints: footprinted - started,
            search: footprinted.elapsed(),
        },
    })
}

/// How many sources a fill packs on no-fit maps: their maps grow with the
/// square of their angles.
const NESTED: usize = 3;

/// Below how many copies the plate holds by area, at the most, a fill does
/// without no-fit maps.
const FEW: f64 = 4.0;

/// `copies` of `problem`'s footprints with more squeezed in, the next
/// copy of each in turn, up to `limit` (#555). `None` when cancelled.
fn squeezed(
    problem: &Problem<'_>,
    copies: Vec<search::Copied>,
    limit: Option<usize>,
    on: bool,
    cancel: &AtomicBool,
) -> Option<Vec<search::Copied>> {
    let n = problem.footprints.len();
    if !on || n == 0 || copies.is_empty() {
        return Some(copies);
    }
    let start = copies.len();
    let placed = copies.into_iter().map(|(k, a, at)| (k, (a, at))).collect();
    let next = (start..).map(|c| c % n);
    let out = crate::separate::squeeze_in(
        problem.footprints,
        &problem.plate,
        placed,
        next,
        |count| limit.is_none_or(|l| count < l),
        cancel,
    )?;
    Some(out.into_iter().map(|(k, (a, at))| (k, a, at)).collect())
}
