/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Worker-side save-progress accounting.
//!
//! A project save runs 3 phases on the worker thread: serialize the
//! scene and slice geometry to XML, compress the assembled package,
//! and write the bytes to disk. Progress is published through one
//! shared `AtomicU32` cell packing `(phase, permille)` with the phase
//! in the high bits, so a phase advance always outranks any within-phase
//! value and `fetch_max` keeps the published value monotonic across the
//! whole save: a straggler tick from an earlier phase can never regress
//! it.
//!
//! [`SaveProgressSink`] is the write side, handed to the worker at
//! dispatch. The UI side polls the cell and decodes it with [`decode`].
//! This module is framework-free, mirroring [`crate::load_progress`];
//! the panel driver that touches Slint lives in
//! [`crate::save_progress_panel`]. The permille scaling is shared with
//! the load-progress accounting.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::load_progress::{PROGRESS_PERMILLE_MAX, permille};

/// Phases of the save pipeline, in execution order. The discriminant
/// is the packed cell's high half, so declaration order is display
/// order and later phases compare greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SavePhase {
    Serialize = 0,
    Compress = 1,
    Writing = 2,
}

/// Packs a phase and a within-phase permille into one cell value. A
/// later phase packed with any permille compares greater than an
/// earlier phase packed with any permille.
fn pack(phase: SavePhase, permille: u32) -> u32 {
    ((phase as u32) << 16) | permille.min(PROGRESS_PERMILLE_MAX)
}

/// Decodes a cell value back into `(phase, permille)`. Values only ever
/// come from [`pack`]; an out-of-range phase half (impossible through
/// the sink) saturates to the last phase.
pub(crate) fn decode(value: u32) -> (SavePhase, u32) {
    let permille = value & 0xFFFF;
    let phase = match value >> 16 {
        0 => SavePhase::Serialize,
        1 => SavePhase::Compress,
        _ => SavePhase::Writing,
    };
    (phase, permille)
}

/// Write side of the shared progress cell. Cheap to clone; clones share
/// the cell. The worker calls [`SaveProgressSink::set`] as its phases
/// advance; [`SaveProgressSink::reset`] rearms the cell at dispatch,
/// before the worker spawns, so a new save never briefly shows the
/// previous save's final value.
#[derive(Clone)]
pub(crate) struct SaveProgressSink {
    cell: Arc<AtomicU32>,
}

impl SaveProgressSink {
    pub(crate) fn new(cell: Arc<AtomicU32>) -> Self {
        Self { cell }
    }

    /// Rearm the cell to the start of the serialize phase.
    pub(crate) fn reset(&self) {
        self.cell.store(0, Ordering::Relaxed);
    }

    /// Record `done` of `total` units inside `phase`. `fetch_max` keeps
    /// the published value monotonic; see the module doc.
    pub(crate) fn set(&self, phase: SavePhase, done: u64, total: u64) {
        self.cell
            .fetch_max(pack(phase, permille(done, total)), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod packing_tests {
    use super::*;

    #[test]
    fn later_phases_outrank_any_earlier_permille() {
        assert!(pack(SavePhase::Compress, 0) > pack(SavePhase::Serialize, PROGRESS_PERMILLE_MAX));
        assert!(pack(SavePhase::Writing, 0) > pack(SavePhase::Compress, PROGRESS_PERMILLE_MAX));
    }

    #[test]
    fn decode_round_trips_every_phase() {
        for phase in [
            SavePhase::Serialize,
            SavePhase::Compress,
            SavePhase::Writing,
        ] {
            for permille in [0, 1, 500, PROGRESS_PERMILLE_MAX] {
                assert_eq!(decode(pack(phase, permille)), (phase, permille));
            }
        }
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    fn sink_and_cell() -> (SaveProgressSink, Arc<AtomicU32>) {
        let cell = Arc::new(AtomicU32::new(0));
        (SaveProgressSink::new(cell.clone()), cell)
    }

    fn published(cell: &AtomicU32) -> (SavePhase, u32) {
        decode(cell.load(Ordering::Relaxed))
    }

    #[test]
    fn set_publishes_phase_and_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(SavePhase::Serialize, 250, 1000);
        assert_eq!(published(&cell), (SavePhase::Serialize, 250));
    }

    #[test]
    fn within_phase_regressions_are_absorbed() {
        let (sink, cell) = sink_and_cell();
        sink.set(SavePhase::Compress, 900, 1000);
        sink.set(SavePhase::Compress, 100, 1000);
        assert_eq!(published(&cell), (SavePhase::Compress, 900));
    }

    #[test]
    fn phase_advance_outranks_a_high_earlier_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(SavePhase::Serialize, 999, 1000);
        sink.set(SavePhase::Compress, 0, 100);
        assert_eq!(published(&cell), (SavePhase::Compress, 0));
    }

    #[test]
    fn reset_rearms_for_the_next_save() {
        let (sink, cell) = sink_and_cell();
        sink.set(SavePhase::Writing, 1, 1);
        sink.reset();
        assert_eq!(published(&cell), (SavePhase::Serialize, 0));
    }
}
