/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Fills the plate with copies (#356).
//!
//! Every body stays where it is, and the others keep clear of it. Copies
//! of some of them, the sources, go in the room left, as many as fit: a
//! copy of each source in turn, so the sources get as many copies each,
//! give or take one. A copy is its source moved in the plate plane, maybe
//! turned about the vertical axis, as [`crate::arrange`] moves a body; it
//! keeps the same gap and edge margin.

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
    pub timings: Timings,
}

/// Finds room on `plate` for copies of the bodies `sources` (indices into
/// `bodies`), around every body, all of which stay where they are: as many
/// as fit, or `limit` at most. `placed` follows how many the search has
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
    let occupancy = frame.plate_with(
        bands_of(footprints.iter().flatten()),
        footprints.iter().flatten(),
    );
    // Sources with no triangles have nothing to copy.
    let sources: Vec<usize> = sources
        .into_iter()
        .filter(|&i| footprints[i].is_some())
        .collect();
    let copied: Vec<Footprint> = sources
        .iter()
        .filter_map(|&i| footprints[i].clone())
        .collect();
    let problem = Problem {
        footprints: &copied,
        plate: occupancy,
        target: [frame.width as i32 / 2, frame.height as i32 / 2],
        cell: frame.geo.cell,
        origin: frame.origin,
        balance: false,
        passes: 0,
        deadline: started,
    };
    let found = search::fill(&problem, limit, placed, cancel).ok_or(Cancelled)?;
    let copies = found
        .into_iter()
        .map(|(k, angle, at)| FillCopy {
            source: sources[k],
            placement: frame.placement(&copied[k], angle, at, [0.0, 0.0]),
        })
        .collect();
    Ok(Filled {
        copies,
        timings: Timings {
            footprints: footprinted - started,
            search: footprinted.elapsed(),
        },
    })
}
