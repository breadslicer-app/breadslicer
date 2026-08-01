/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Worker-side export-progress accounting.
//!
//! A scene export runs up to 4 phases on the worker thread: flatten
//! (STL formats merge the scene into one mesh), serialize, compress
//! (3MF only), and the final disk write. Progress is published through
//! one shared `AtomicU32` cell packing `(phase, permille)` with the
//! phase in the high bits, so a phase advance always outranks any
//! within-phase value and `fetch_max` keeps the published value
//! monotonic across the whole export: a straggler tick from an earlier
//! phase can never regress it.
//!
//! [`ExportProgressSink`] is the write side, handed to the worker at
//! dispatch. The UI side polls the cell and decodes it with
//! [`decode`]. This module is framework-free, mirroring
//! [`crate::save_progress`]; the panel driver that touches Slint lives
//! in [`crate::export_progress_panel`]. The permille scaling is shared
//! with the load-progress accounting.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::load_progress::{PROGRESS_PERMILLE_MAX, permille};

/// Phases of the export pipeline, in execution order. The discriminant
/// is the packed cell's high half, so declaration order is display
/// order and later phases compare greater. Not every format visits
/// every phase; skipped phases simply never publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportPhase {
    Flatten = 0,
    Serialize = 1,
    Compress = 2,
    Write = 3,
}

/// Packs a phase and a within-phase permille into one cell value. A
/// later phase packed with any permille compares greater than an
/// earlier phase packed with any permille.
fn pack(phase: ExportPhase, permille: u32) -> u32 {
    ((phase as u32) << 16) | permille.min(PROGRESS_PERMILLE_MAX)
}

/// Decodes a cell value back into `(phase, permille)`. Values only ever
/// come from [`pack`]; an out-of-range phase half (impossible through
/// the sink) saturates to the last phase.
pub(crate) fn decode(value: u32) -> (ExportPhase, u32) {
    let permille = value & 0xFFFF;
    let phase = match value >> 16 {
        0 => ExportPhase::Flatten,
        1 => ExportPhase::Serialize,
        2 => ExportPhase::Compress,
        _ => ExportPhase::Write,
    };
    (phase, permille)
}

/// Write side of the shared progress cell. Cheap to clone; clones share
/// the cell. The worker calls [`ExportProgressSink::set`] as its phases
/// advance; [`ExportProgressSink::reset`] rearms the cell at dispatch,
/// before the worker spawns, so a new export never briefly shows the
/// previous export's final value.
#[derive(Clone)]
pub(crate) struct ExportProgressSink {
    cell: Arc<AtomicU32>,
}

impl ExportProgressSink {
    pub fn new(cell: Arc<AtomicU32>) -> Self {
        Self { cell }
    }

    /// Rearm the cell to the start of the flatten phase.
    pub fn reset(&self) {
        self.cell.store(0, Ordering::Relaxed);
    }

    /// Record `done` of `total` units inside `phase`. `fetch_max` keeps
    /// the published value monotonic; see the module doc.
    pub fn set(&self, phase: ExportPhase, done: u64, total: u64) {
        self.cell
            .fetch_max(pack(phase, permille(done, total)), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod packing_tests {
    use super::*;

    #[test]
    fn later_phases_outrank_any_earlier_permille() {
        assert!(
            pack(ExportPhase::Serialize, 0) > pack(ExportPhase::Flatten, PROGRESS_PERMILLE_MAX)
        );
        assert!(
            pack(ExportPhase::Compress, 0) > pack(ExportPhase::Serialize, PROGRESS_PERMILLE_MAX)
        );
        assert!(pack(ExportPhase::Write, 0) > pack(ExportPhase::Compress, PROGRESS_PERMILLE_MAX));
    }

    #[test]
    fn decode_round_trips_every_phase() {
        for phase in [
            ExportPhase::Flatten,
            ExportPhase::Serialize,
            ExportPhase::Compress,
            ExportPhase::Write,
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

    fn sink_and_cell() -> (ExportProgressSink, Arc<AtomicU32>) {
        let cell = Arc::new(AtomicU32::new(0));
        (ExportProgressSink::new(cell.clone()), cell)
    }

    fn published(cell: &AtomicU32) -> (ExportPhase, u32) {
        decode(cell.load(Ordering::Relaxed))
    }

    #[test]
    fn set_publishes_phase_and_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(ExportPhase::Serialize, 250, 1000);
        assert_eq!(published(&cell), (ExportPhase::Serialize, 250));
    }

    #[test]
    fn within_phase_regressions_are_absorbed() {
        let (sink, cell) = sink_and_cell();
        sink.set(ExportPhase::Compress, 900, 1000);
        sink.set(ExportPhase::Compress, 100, 1000);
        assert_eq!(published(&cell), (ExportPhase::Compress, 900));
    }

    #[test]
    fn phase_advance_outranks_a_high_earlier_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(ExportPhase::Flatten, 999, 1000);
        sink.set(ExportPhase::Serialize, 0, 100);
        assert_eq!(published(&cell), (ExportPhase::Serialize, 0));
    }

    #[test]
    fn reset_rearms_for_the_next_export() {
        let (sink, cell) = sink_and_cell();
        sink.set(ExportPhase::Write, 1, 1);
        sink.reset();
        assert_eq!(published(&cell), (ExportPhase::Flatten, 0));
    }
}
