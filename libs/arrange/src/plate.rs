/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The plate as a grid per height band, with what is already placed on it.

use crate::footprint::{BandMask, Shape};
use crate::grid::BitGrid;

/// What the plate holds, band by band. A band nothing reaches has no grid.
#[derive(Clone, Debug)]
pub(crate) struct Occupancy {
    pub(crate) width: usize,
    pub(crate) height: usize,
    bands: Vec<Option<BitGrid>>,
}

impl Occupancy {
    pub(crate) fn new(width: usize, height: usize, bands: usize) -> Self {
        Self {
            width,
            height,
            bands: vec![None; bands],
        }
    }

    /// Whether `shape` with its frame's corner at plate cell `at` lies on
    /// the plate.
    pub(crate) fn holds(&self, shape: &Shape, at: [i32; 2]) -> bool {
        at[0] + shape.lo[0] >= 0
            && at[1] + shape.lo[1] >= 0
            && at[0] + shape.hi[0] <= self.width as i32
            && at[1] + shape.hi[1] <= self.height as i32
    }

    /// Whether `shape` there shares a cell with what is placed, in any band.
    /// It must lie on the plate.
    pub(crate) fn collides(&self, shape: &Shape, at: [i32; 2]) -> bool {
        shape.bands.iter().any(|mask| {
            self.bands
                .get(mask.band)
                .and_then(Option::as_ref)
                .is_some_and(|grid| hits(grid, mask, at))
        })
    }

    /// Whether `shape` can go there: on the plate, clear of what is placed.
    pub(crate) fn fits(&self, shape: &Shape, at: [i32; 2]) -> bool {
        self.holds(shape, at) && !self.collides(shape, at)
    }

    /// Places `shape` there. What falls off the plate is dropped: a body
    /// left where it stands may reach past the edge.
    pub(crate) fn stamp(&mut self, shape: &Shape, at: [i32; 2]) {
        let (width, height) = (self.width, self.height);
        for mask in &shape.bands {
            let Some(slot) = self.bands.get_mut(mask.band) else {
                continue;
            };
            let grid = slot.get_or_insert_with(|| BitGrid::new(width, height));
            let (q, s) = word_shift(at[0] + mask.x0);
            for &r in &mask.rows {
                let py = at[1] + mask.y0 + r as i32;
                if py < 0 || py as usize >= height {
                    continue;
                }
                let plate = grid.row_mut(py as usize);
                let len = plate.len() as isize;
                for (k, &w) in mask.grid.row(r as usize).iter().enumerate() {
                    if w == 0 {
                        continue;
                    }
                    let i = q + k as isize;
                    if (0..len).contains(&i) {
                        plate[i as usize] |= w << s;
                    }
                    if s > 0 && (0..len).contains(&(i + 1)) {
                        plate[(i + 1) as usize] |= w >> (64 - s);
                    }
                }
            }
        }
    }
}

/// The plate word a mask's first word lands on, which may be before the
/// first, and how many bits into it.
fn word_shift(x: i32) -> (isize, u32) {
    (x.div_euclid(64) as isize, x.rem_euclid(64) as u32)
}

/// Whether `mask` with its body's frame at `at` shares a cell with `grid`.
fn hits(grid: &BitGrid, mask: &BandMask, at: [i32; 2]) -> bool {
    let (q, s) = word_shift(at[0] + mask.x0);
    let y = at[1] + mask.y0;
    for &r in &mask.rows {
        let py = y + r as i32;
        if py < 0 || py as usize >= grid.height {
            continue;
        }
        let plate = grid.row(py as usize);
        let len = plate.len() as isize;
        for (k, &w) in mask.grid.row(r as usize).iter().enumerate() {
            if w == 0 {
                continue;
            }
            let i = q + k as isize;
            if (0..len).contains(&i) && plate[i as usize] & (w << s) != 0 {
                return true;
            }
            if s > 0
                && (0..len).contains(&(i + 1))
                && plate[(i + 1) as usize] & (w >> (64 - s)) != 0
            {
                return true;
            }
        }
    }
    false
}
