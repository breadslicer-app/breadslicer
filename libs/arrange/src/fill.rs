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
//! the one with more copies wins; on a tie the sources stay.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::footprint::Footprint;
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
    let problem = |plate| Problem {
        footprints: &copied,
        plate,
        target: [frame.width as i32 / 2, frame.height as i32 / 2],
        cell: frame.geo.cell,
        origin: frame.origin,
        balance: false,
        passes: 0,
        deadline: started,
    };
    let staying = problem(frame.plate_with(bands, footprints.iter().flatten()));
    let others = footprints
        .iter()
        .enumerate()
        .filter(|(i, _)| sources.binary_search(i).is_err())
        .filter_map(|(_, fp)| fp.as_ref());
    let moving = problem(frame.plate_with(bands, others));
    // With the sources moving, the first of each source's places is its
    // own, and the copies come on top of them.
    let n = sources.len();
    let (stayed, moved) = rayon::join(
        || search::fill(&staying, limit, placed, 0, cancel),
        || search::fill(&moving, limit.map(|l| l + n), placed, n, cancel),
    );
    let (stayed, moved) = (stayed.ok_or(Cancelled)?, moved.ok_or(Cancelled)?);
    let all_placed = (0..n).all(|k| moved.iter().any(|&(c, _, _)| c == k));
    let to_fill = |(k, angle, at): search::Copied| FillCopy {
        source: sources[k],
        placement: frame.placement(&copied[k], angle, at, [0.0, 0.0]),
    };
    let (copies, moved): (Vec<FillCopy>, Vec<FillCopy>) =
        if all_placed && moved.len() - n > stayed.len() {
            let mut own = vec![false; n];
            let (mut copies, mut originals) = (Vec::new(), Vec::new());
            for found in moved {
                if own[found.0] {
                    copies.push(to_fill(found));
                } else {
                    own[found.0] = true;
                    originals.push(to_fill(found));
                }
            }
            (copies, originals)
        } else {
            (stayed.into_iter().map(to_fill).collect(), Vec::new())
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
